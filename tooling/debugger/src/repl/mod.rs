mod repl_debugger;

use acvm::acir::circuit::OpcodeLocation;
use acvm::acir::native_types::WitnessMap;

use nargo::{artifacts::debug::DebugArtifact, NargoError};

use noirc_driver::CompiledProgram;

use mini_async_repl::{
    anyhow,
    command::{
        lift_validation_err, validate, Command, CommandArgInfo, CommandArgType, ExecuteCommand,
    },
    repl::LoopStatus,
    CommandStatus, Repl,
};

use tokio::{
    sync::{mpsc, mpsc::Receiver, mpsc::Sender},
    task::{spawn_local, LocalSet},
};

use std::future::Future;
use std::pin::Pin;

use repl_debugger::ReplDebugger;

pub fn run(
    compiled_program: &CompiledProgram,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    let compiled_program = compiled_program.clone();

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    // We use a single threaded environment to relax a bit the demands on our type bounds.
    let local = LocalSet::new();

    rt.block_on(async {
        local
            .run_until(async move {
                let (to_debugger, debugger_inbox) = mpsc::channel::<ReplDebuggerMessage>(1);
                let (debugger_outbox, from_debugger) = mpsc::channel::<Option<WitnessMap>>(1);

                // Debugger CLI core:
                // - interacts with debugger runtime and prints to terminal
                // - receives ReplDebuggerMessage's
                // - emits a WitnessMap if the circuit succeeds
                let debugger = spawn_debugger(
                    debugger_inbox,
                    debugger_outbox,
                    compiled_program,
                    initial_witness,
                );

                // Debugger REPL loop:
                // - sets up available REPL commands
                // - translates user input to debugger commands
                // - emits ReplDebuggerMessage's to drive the debugger
                let repl = spawn_repl(to_debugger, from_debugger);

                debugger.await.unwrap();
                match repl.await {
                    Ok(r) => Ok(r),
                    Err(_e) => Ok(None),
                }
            })
            .await
    })
}

fn spawn_debugger(
    mut debugger_inbox: Receiver<ReplDebuggerMessage>,
    debugger_outbox: Sender<Option<WitnessMap>>,
    compiled_program: CompiledProgram,
    initial_witness: WitnessMap,
) -> tokio::task::JoinHandle<()> {
    spawn_local(async move {
        #[allow(deprecated)]
        let blackbox_solver = barretenberg_blackbox_solver::BarretenbergSolver::new();

        let debug_artifact = DebugArtifact {
            debug_symbols: vec![compiled_program.debug.clone()],
            file_map: compiled_program.file_map.clone(),
            warnings: compiled_program.warnings.clone(),
        };

        let mut repl_debugger = ReplDebugger::new(
            &blackbox_solver,
            &compiled_program.circuit,
            &debug_artifact,
            initial_witness,
        );

        repl_debugger.show_current_vm_status();

        while let Some(msg) = debugger_inbox.recv().await {
            match msg {
                ReplDebuggerMessage::StepIntoOpcode => {
                    repl_debugger.step_into_opcode().await;
                }
                ReplDebuggerMessage::StepIntoACIR => {
                    repl_debugger.step_acir_opcode().await;
                }
                ReplDebuggerMessage::Next => {
                    repl_debugger.next().await;
                }
                ReplDebuggerMessage::Continue => {
                    repl_debugger.cont().await;
                }
                ReplDebuggerMessage::Restart => {
                    repl_debugger.restart_session();
                }
                ReplDebuggerMessage::DisplayACIROpcodes => {
                    repl_debugger.display_opcodes();
                }
                ReplDebuggerMessage::ShowWitnessMap => {
                    repl_debugger.show_witness_map();
                }
                ReplDebuggerMessage::ShowBrilligRegisters => {
                    repl_debugger.show_brillig_registers();
                }
                ReplDebuggerMessage::ShowBrilligMemory => {
                    repl_debugger.show_brillig_memory();
                }
                ReplDebuggerMessage::DisplaySingleWitness { index } => {
                    repl_debugger.show_witness(index);
                }
                ReplDebuggerMessage::UpdateWitness { index, new_value } => {
                    repl_debugger.update_witness(index, new_value);
                }
                ReplDebuggerMessage::RegSet { index, new_value } => {
                    repl_debugger.set_brillig_register(index, new_value);
                }
                ReplDebuggerMessage::MemSet { index, new_value } => {
                    repl_debugger.write_brillig_memory(index, new_value);
                }
                ReplDebuggerMessage::BreakpointSet { location } => {
                    repl_debugger.add_breakpoint_at(location);
                }
                ReplDebuggerMessage::BreakpointDelete { location } => {
                    repl_debugger.delete_breakpoint_at(location);
                }
                ReplDebuggerMessage::Finalize => {
                    if repl_debugger.is_solved() {
                        debugger_outbox.send(Some(repl_debugger.get_witness_map())).await;
                    } else {
                        debugger_outbox.send(None).await;
                    }
                }
            }
        }
    })
}

fn spawn_repl(
    to_debugger: Sender<ReplDebuggerMessage>,
    mut from_debugger: Receiver<Option<WitnessMap>>,
) -> tokio::task::JoinHandle<Option<WitnessMap>> {
    spawn_local(async move {
        let mut repl = Repl::builder()
            .add(
                "step",
                Command::new(
                    "step to the next ACIR opcode",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::StepIntoACIR,
                    )),
                ),
            )
            .add(
                "into",
                Command::new(
                    "step into to the next opcode",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::StepIntoOpcode,
                    )),
                ),
            )
            .add(
                "next",
                Command::new(
                    "step until a new source location is reached",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::Next,
                    )),
                ),
            )
            .add(
                "continue",
                Command::new(
                    "continue execution until the end of the program",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::Continue,
                    )),
                ),
            )
            .add(
                "restart",
                Command::new(
                    "restart the debugging session",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::Restart,
                    )),
                ),
            )
            .add(
                "opcodes",
                Command::new(
                    "display ACIR opcodes",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::DisplayACIROpcodes,
                    )),
                ),
            )
            .add(
                "witness",
                Command::new(
                    "show witness map",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::ShowWitnessMap,
                    )),
                ),
            )
            .add(
                "registers",
                Command::new(
                    "show Brillig registers (valid when executing a Brillig block)",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::ShowBrilligRegisters,
                    )),
                ),
            )
            .add(
                "memory",
                Command::new(
                    "show Brillig memory (valid when executing a Brillig block)",
                    vec![],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::ShowBrilligMemory,
                    )),
                ),
            )
            .add(
                "witness",
                Command::new(
                    "display a single witness from the witness map",
                    vec![CommandArgInfo::new_with_name(CommandArgType::I32, "index")],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::DisplaySingleWitness,
                    )),
                ),
            )
            .add(
                "witness",
                Command::new(
                    "update a witness with the given value",
                    vec![
                        CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                        CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                    ],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::UpdateWitness,
                    )),
                ),
            )
            .add(
                "regset",
                Command::new(
                    "update a Brillig register with the given value",
                    vec![
                        CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                        CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                    ],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::RegSet,
                    )),
                ),
            )
            .add(
                "memset",
                Command::new(
                    "update a Brillig memory cell with the given value",
                    vec![
                        CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                        CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                    ],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::MemSet,
                    )),
                ),
            )
            .add(
                "break",
                Command::new(
                    "add a breakpoint at an opcode location",
                    vec![CommandArgInfo::new_with_name(CommandArgType::Custom, "location")],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::BreakpointSet,
                    )),
                ),
            )
            .add(
                "delete",
                Command::new(
                    "delete breakpoint at an opcode location",
                    vec![CommandArgInfo::new_with_name(CommandArgType::Custom, "location")],
                    Box::new(DebuggerCommandHandler::new(
                        to_debugger.clone(),
                        ReplDebuggerMessageType::BreakpointDelete,
                    )),
                ),
            )
            .build()
            .expect("Debugger error");

        let mut repl_status = LoopStatus::Continue;
        while repl_status == LoopStatus::Continue {
            repl_status = repl.next().await.expect("Debugger REPL error");

            //TODO: we probably need to change mini_async_repl's
            //blocking readline to avoid this
            tokio::task::yield_now().await;
        }

        to_debugger.send(ReplDebuggerMessage::Finalize).await;

        if let Some(debugger_msg) = from_debugger.recv().await {
            return debugger_msg;
        }

        None
    })
}

struct DebuggerCommandHandler {
    debugger_msg_type: ReplDebuggerMessageType,
    debugger: Sender<ReplDebuggerMessage>,
}
impl DebuggerCommandHandler {
    pub fn new(
        debugger: Sender<ReplDebuggerMessage>,
        debugger_msg_type: ReplDebuggerMessageType,
    ) -> Self {
        Self { debugger, debugger_msg_type }
    }
    async fn handle_command(
        &mut self,
        debugger_msg: ReplDebuggerMessage,
    ) -> anyhow::Result<CommandStatus> {
        {
            self.debugger.send(debugger_msg).await;
            anyhow::Ok(CommandStatus::Done)
        }
    }
}
impl ExecuteCommand for DebuggerCommandHandler {
    fn execute(
        &mut self,
        args: Vec<String>,
        args_info: Vec<CommandArgInfo>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CommandStatus>> + '_>> {
        let valid = validate(args.clone(), args_info.clone());
        if let Err(e) = valid {
            return Box::pin(lift_validation_err(Err(e)));
        }

        let debugger_msg = match self.debugger_msg_type {
            ReplDebuggerMessageType::StepIntoOpcode => ReplDebuggerMessage::StepIntoOpcode,
            ReplDebuggerMessageType::StepIntoACIR => ReplDebuggerMessage::StepIntoACIR,
            ReplDebuggerMessageType::Next => ReplDebuggerMessage::Next,
            ReplDebuggerMessageType::Continue => ReplDebuggerMessage::Continue,
            ReplDebuggerMessageType::Restart => ReplDebuggerMessage::Restart,
            ReplDebuggerMessageType::DisplayACIROpcodes => ReplDebuggerMessage::DisplayACIROpcodes,
            ReplDebuggerMessageType::ShowWitnessMap => ReplDebuggerMessage::ShowWitnessMap,
            ReplDebuggerMessageType::ShowBrilligRegisters => {
                ReplDebuggerMessage::ShowBrilligRegisters
            }
            ReplDebuggerMessageType::ShowBrilligMemory => ReplDebuggerMessage::ShowBrilligMemory,
            ReplDebuggerMessageType::DisplaySingleWitness => {
                let index = args[0].parse::<u32>();

                match index {
                    Ok(index) => ReplDebuggerMessage::DisplaySingleWitness { index },
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
            ReplDebuggerMessageType::UpdateWitness => {
                let index = args[0].parse::<u32>();

                match index {
                    Ok(index) => {
                        ReplDebuggerMessage::UpdateWitness { index, new_value: args[1].clone() }
                    }
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
            ReplDebuggerMessageType::RegSet => {
                let index = args[0].parse::<usize>();

                match index {
                    Ok(index) => ReplDebuggerMessage::RegSet { index, new_value: args[1].clone() },
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
            ReplDebuggerMessageType::MemSet => {
                let index = args[0].parse::<usize>();

                match index {
                    Ok(index) => ReplDebuggerMessage::MemSet { index, new_value: args[1].clone() },
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
            ReplDebuggerMessageType::BreakpointSet => {
                let location = args[0].parse::<OpcodeLocation>();
                match location {
                    Ok(location) => ReplDebuggerMessage::BreakpointSet { location },
                    // TODO: not true, I think
                    // TODO: check how params are displayed now in repl, and fix mini_async_repl
                    // if necessary
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
            ReplDebuggerMessageType::BreakpointDelete => {
                let location = args[0].parse::<OpcodeLocation>();
                match location {
                    Ok(location) => ReplDebuggerMessage::BreakpointDelete { location },
                    _ => panic!("Unreachable, validator should have covered this"),
                }
            }
        };

        Box::pin(self.handle_command(debugger_msg))
    }
}

#[derive(Debug, Clone)]
enum ReplDebuggerMessageType {
    StepIntoOpcode,
    StepIntoACIR,
    Next,
    Continue,
    Restart,
    DisplayACIROpcodes,
    ShowWitnessMap,
    ShowBrilligRegisters,
    ShowBrilligMemory,
    DisplaySingleWitness,
    UpdateWitness,
    RegSet,
    MemSet,
    BreakpointSet,
    BreakpointDelete,
}

#[derive(Debug, Clone)]
enum ReplDebuggerMessage {
    StepIntoOpcode,
    StepIntoACIR,
    Next,
    Continue,
    Restart,
    DisplayACIROpcodes,
    ShowWitnessMap,
    ShowBrilligRegisters,
    ShowBrilligMemory,
    DisplaySingleWitness { index: u32 },
    UpdateWitness { index: u32, new_value: String },
    RegSet { index: usize, new_value: String },
    MemSet { index: usize, new_value: String },
    BreakpointSet { location: OpcodeLocation },
    BreakpointDelete { location: OpcodeLocation },
    Finalize,
}
