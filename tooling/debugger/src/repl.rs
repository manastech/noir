use crate::context::{DebugCommandResult, DebugContext};

use acvm::acir::circuit::{Circuit, Opcode, OpcodeLocation};
use acvm::acir::native_types::{Witness, WitnessMap};
use acvm::{BlackBoxFunctionSolver, FieldElement};

use nargo::{artifacts::debug::DebugArtifact, ops::DefaultForeignCallExecutor, NargoError};

use mini_async_repl::{
    anyhow,
    command::{
        lift_validation_err, validate, Command, CommandArgInfo, CommandArgType, ExecuteCommand,
    },
    repl::LoopStatus,
    CommandStatus, Repl,
};

use codespan_reporting::files::Files;
use noirc_errors::Location;

use owo_colors::OwoColorize;

use std::ops::Range;

use acvm::pwg::{
    ACVMStatus, BrilligSolver, BrilligSolverStatus, ForeignCallWaitInfo, StepResult, ACVM,
};

use acvm::brillig_vm::{brillig::ForeignCallResult, brillig::Value, Registers};

use acvm::acir::{
    brillig::{Opcode as BrilligOpcode, RegisterIndex, RegisterOrMemory},
    circuit::brillig::{Brillig, BrilligInputs},
    native_types::Expression,
};

use noirc_driver::{CompileOptions, CompiledProgram, NOIR_ARTIFACT_VERSION_STRING};

use tokio::sync::mpsc::Sender;

use std::pin::Pin;

use std::future::Future;

pub struct ReplDebugger<'a, B: BlackBoxFunctionSolver> {
    context: DebugContext<'a, B>,
    blackbox_solver: &'a B,
    circuit: &'a Circuit,
    debug_artifact: &'a DebugArtifact,
    initial_witness: WitnessMap,
    last_result: DebugCommandResult,
    outbox: Sender<Option<WitnessMap>>,
}

impl<'a, B: BlackBoxFunctionSolver> ReplDebugger<'a, B> {
    pub fn new(
        blackbox_solver: &'a B,
        circuit: &'a Circuit,
        debug_artifact: &'a DebugArtifact,
        initial_witness: WitnessMap,
        outbox: Sender<Option<WitnessMap>>,
    ) -> Self {
        let context = DebugContext::new(
            blackbox_solver,
            circuit,
            debug_artifact,
            initial_witness.clone(),
            Box::new(DefaultForeignCallExecutor::new(true)),
        );
        Self {
            context,
            blackbox_solver,
            circuit,
            debug_artifact,
            initial_witness,
            last_result: DebugCommandResult::Ok,
            outbox,
        }
    }

    pub fn show_current_vm_status(&self) {
        let location = self.context.get_current_opcode_location();
        let opcodes = self.context.get_opcodes();

        match location {
            None => println!("Finished execution"),
            Some(location) => {
                match location {
                    OpcodeLocation::Acir(ip) => {
                        println!("At opcode {}: {}", ip, opcodes[ip])
                    }
                    OpcodeLocation::Brillig { acir_index, brillig_index } => {
                        let Opcode::Brillig(ref brillig) = opcodes[acir_index] else {
                            unreachable!("Brillig location does not contain a Brillig block");
                        };
                        println!(
                            "At opcode {}.{}: {:?}",
                            acir_index, brillig_index, brillig.bytecode[brillig_index]
                        );
                    }
                }
                self.show_source_code_location(&location);
            }
        }
    }

    fn print_location_path(&self, loc: Location) {
        let line_number = self.debug_artifact.location_line_number(loc).unwrap();
        let column_number = self.debug_artifact.location_column_number(loc).unwrap();

        println!(
            "At {}:{line_number}:{column_number}",
            self.debug_artifact.name(loc.file).unwrap()
        );
    }

    fn show_source_code_location(&self, location: &OpcodeLocation) {
        let locations = self.debug_artifact.debug_symbols[0].opcode_location(location);
        let Some(locations) = locations else { return };
        for loc in locations {
            self.print_location_path(loc);

            let loc_line_index = self.debug_artifact.location_line_index(loc).unwrap();

            // How many lines before or after the location's line we
            // print
            let context_lines = 5;

            let first_line_to_print =
                if loc_line_index < context_lines { 0 } else { loc_line_index - context_lines };

            let last_line_index = self.debug_artifact.last_line_index(loc).unwrap();
            let last_line_to_print = std::cmp::min(loc_line_index + context_lines, last_line_index);

            let source = self.debug_artifact.location_source_code(loc).unwrap();
            for (current_line_index, line) in source.lines().enumerate() {
                let current_line_number = current_line_index + 1;

                if current_line_index < first_line_to_print {
                    // Ignore lines before range starts
                    continue;
                } else if current_line_index == first_line_to_print && current_line_index > 0 {
                    // Denote that there's more lines before but we're not showing them
                    print_line_of_ellipsis(current_line_index);
                }

                if current_line_index > last_line_to_print {
                    // Denote that there's more lines after but we're not showing them,
                    // and stop printing
                    print_line_of_ellipsis(current_line_number);
                    break;
                }

                if current_line_index == loc_line_index {
                    // Highlight current location
                    let Range { start: loc_start, end: loc_end } =
                        self.debug_artifact.location_in_line(loc).unwrap();
                    println!(
                        "{:>3} {:2} {}{}{}",
                        current_line_number,
                        "->",
                        &line[0..loc_start].to_string().dimmed(),
                        &line[loc_start..loc_end],
                        &line[loc_end..].to_string().dimmed()
                    );
                } else {
                    print_dimmed_line(current_line_number, line);
                }
            }
        }
    }

    fn display_opcodes(&self) {
        let opcodes = self.context.get_opcodes();
        let current_opcode_location = self.context.get_current_opcode_location();
        let current_acir_index = match current_opcode_location {
            Some(OpcodeLocation::Acir(ip)) => Some(ip),
            Some(OpcodeLocation::Brillig { acir_index, .. }) => Some(acir_index),
            None => None,
        };
        let current_brillig_index = match current_opcode_location {
            Some(OpcodeLocation::Brillig { brillig_index, .. }) => brillig_index,
            _ => 0,
        };
        let outer_marker = |acir_index| {
            if current_acir_index == Some(acir_index) {
                "->"
            } else if self.context.is_breakpoint_set(&OpcodeLocation::Acir(acir_index)) {
                " *"
            } else {
                ""
            }
        };
        let brillig_marker = |acir_index, brillig_index| {
            if current_acir_index == Some(acir_index) && brillig_index == current_brillig_index {
                "->"
            } else if self
                .context
                .is_breakpoint_set(&OpcodeLocation::Brillig { acir_index, brillig_index })
            {
                " *"
            } else {
                ""
            }
        };
        for (acir_index, opcode) in opcodes.iter().enumerate() {
            let marker = outer_marker(acir_index);
            if let Opcode::Brillig(brillig) = opcode {
                println!("{:>3} {:2} BRILLIG inputs={:?}", acir_index, marker, brillig.inputs);
                println!("       |       outputs={:?}", brillig.outputs);
                for (brillig_index, brillig_opcode) in brillig.bytecode.iter().enumerate() {
                    println!(
                        "{:>3}.{:<2} |{:2} {:?}",
                        acir_index,
                        brillig_index,
                        brillig_marker(acir_index, brillig_index),
                        brillig_opcode
                    );
                }
            } else {
                println!("{:>3} {:2} {:?}", acir_index, marker, opcode);
            }
        }
    }

    fn add_breakpoint_at(&mut self, location: OpcodeLocation) {
        if !self.context.is_valid_opcode_location(&location) {
            println!("Invalid opcode location {location}");
        } else if self.context.add_breakpoint(location) {
            println!("Added breakpoint at opcode {location}");
        } else {
            println!("Breakpoint at opcode {location} already set");
        }
    }

    fn delete_breakpoint_at(&mut self, location: OpcodeLocation) {
        if self.context.delete_breakpoint(&location) {
            println!("Breakpoint at opcode {location} deleted");
        } else {
            println!("Breakpoint at opcode {location} not set");
        }
    }

    fn validate_in_progress(&self) -> bool {
        match self.last_result {
            DebugCommandResult::Ok | DebugCommandResult::BreakpointReached(..) => true,
            DebugCommandResult::Done => {
                println!("Execution finished");
                false
            }
            DebugCommandResult::Error(ref error) => {
                println!("ERROR: {}", error);
                self.show_current_vm_status();
                false
            }
        }
    }

    fn handle_debug_command_result(&mut self, result: DebugCommandResult) {
        match &result {
            DebugCommandResult::BreakpointReached(location) => {
                println!("Stopped at breakpoint in opcode {}", location);
            }
            DebugCommandResult::Error(error) => {
                println!("ERROR: {}", error);
            }
            _ => (),
        }
        self.last_result = result;
        self.show_current_vm_status();
    }

    async fn step_acir_opcode(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.step_acir_opcode().await;
            self.handle_debug_command_result(result);
        }
    }

    async fn step_into_opcode(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.step_into_opcode().await;
            self.handle_debug_command_result(result);
        }
    }

    async fn next(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.next().await;
            self.handle_debug_command_result(result);
        }
    }

    async fn cont(&mut self) {
        if self.validate_in_progress() {
            println!("(Continuing execution...)");
            let result = self.context.cont().await;
            self.handle_debug_command_result(result);
        }
    }

    fn restart_session(&mut self) {
        let breakpoints: Vec<OpcodeLocation> =
            self.context.iterate_breakpoints().copied().collect();
        self.context = DebugContext::new(
            self.blackbox_solver,
            self.circuit,
            self.debug_artifact,
            self.initial_witness.clone(),
            Box::new(DefaultForeignCallExecutor::new(true)),
        );
        for opcode_location in breakpoints {
            self.context.add_breakpoint(opcode_location);
        }
        self.last_result = DebugCommandResult::Ok;
        println!("Restarted debugging session.");
        self.show_current_vm_status();
    }

    pub fn get_witness_map(&self) -> WitnessMap {
        self.context.get_witness_map().clone()
    }

    pub fn show_witness_map(&self) {
        let witness_map = self.context.get_witness_map();
        // NOTE: we need to clone() here to get the iterator
        for (witness, value) in witness_map.clone().into_iter() {
            println!("_{} = {value}", witness.witness_index());
        }
    }

    pub fn show_witness(&self, index: u32) {
        if let Some(value) = self.context.get_witness_map().get_index(index) {
            println!("_{} = {value}", index);
        }
    }

    pub fn update_witness(&mut self, index: u32, value: String) {
        let Some(field_value) = FieldElement::try_from_str(&value) else {
            println!("Invalid witness value: {value}");
            return;
        };

        let witness = Witness::from(index);
        _ = self.context.overwrite_witness(witness, field_value);
        println!("_{} = {value}", index);
    }

    pub fn show_brillig_registers(&self) {
        if !self.context.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }

        let Some(registers) = self.context.get_brillig_registers() else {
            // this can happen when just entering the Brillig block since ACVM
            // would have not initialized the Brillig VM yet; in fact, the
            // Brillig code may be skipped altogether
            println!("Brillig VM registers not available");
            return;
        };

        for (index, value) in registers.inner.iter().enumerate() {
            println!("{index} = {}", value.to_field());
        }
    }

    pub fn set_brillig_register(&mut self, index: usize, value: String) {
        let Some(field_value) = FieldElement::try_from_str(&value) else {
            println!("Invalid value: {value}");
            return;
        };
        if !self.context.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }
        self.context.set_brillig_register(index, field_value);
    }

    pub fn show_brillig_memory(&self) {
        if !self.context.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }

        let Some(memory) = self.context.get_brillig_memory() else {
            // this can happen when just entering the Brillig block since ACVM
            // would have not initialized the Brillig VM yet; in fact, the
            // Brillig code may be skipped altogether
            println!("Brillig VM memory not available");
            return;
        };

        for (index, value) in memory.iter().enumerate() {
            println!("{index} = {}", value.to_field());
        }
    }

    pub fn write_brillig_memory(&mut self, index: usize, value: String) {
        let Some(field_value) = FieldElement::try_from_str(&value) else {
            println!("Invalid value: {value}");
            return;
        };
        if !self.context.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }
        self.context.write_brillig_memory(index, field_value);
    }

    fn is_solved(&self) -> bool {
        self.context.is_solved()
    }

    fn finalize(self) -> WitnessMap {
        self.context.finalize()
    }
}

fn print_line_of_ellipsis(line_number: usize) {
    println!("{}", format!("{:>3} {}", line_number, "...").dimmed());
}

fn print_dimmed_line(line_number: usize, line: &str) {
    println!("{}", format!("{:>3} {:2} {}", line_number, "", line).dimmed());
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
            ReplDebuggerMessageType::Finalize => ReplDebuggerMessage::Finalize,
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
    Finalize,
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

pub fn run(
    compiled_program: &CompiledProgram,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    use tokio::sync::mpsc;
    use tokio::sync::oneshot;
    use tokio::task::spawn_local;
    use tokio::task::LocalSet;

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    // Using single threaded environment to
    // relax a bit the demands on our type bounds
    let local = LocalSet::new();

    let compiled_program = compiled_program.clone();

    rt.block_on(async {
        let result =
            local
                .run_until(async move {
                    let (to_debugger, mut debugger_inbox) = mpsc::channel::<ReplDebuggerMessage>(1);
                    let (from_debugger, mut receive_from_debugger) =
                        mpsc::channel::<Option<WitnessMap>>(1);

                    let debugger = spawn_local(async move {
                        let blackbox_solver =
                            barretenberg_blackbox_solver::BarretenbergSolver::new();

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
                            from_debugger.clone(),
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
                                        from_debugger
                                            .send(Some(repl_debugger.get_witness_map()))
                                            .await;
                                    } else {
                                        from_debugger.send(None).await;
                                    }
                                }
                            }
                        }
                    });

                    let repl =
                        spawn_local(async move {
                            let mut repl = Repl::builder()
                    .add("step", Command::new(
                        "step to the next ACIR opcode",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::StepIntoACIR
                            )
                        )
                    ))
                    .add("into", Command::new(
                        "step into to the next opcode",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::StepIntoOpcode
                            )
                        )
                    ))
                    .add("next", Command::new(
                        "step until a new source location is reached",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::Next
                            )
                        )
                    ))
                    .add("continue", Command::new(
                        "continue execution until the end of the program",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::Continue
                            )
                        )
                    ))
                    .add("restart", Command::new(
                        "restart the debugging session",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::Restart
                            )
                        )
                    ))
                    .add("opcodes", Command::new(
                        "display ACIR opcodes",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::DisplayACIROpcodes
                            )
                        )
                    ))
                    .add("witness", Command::new(
                        "show witness map",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::ShowWitnessMap
                            )
                        )
                    ))
                    .add("registers", Command::new(
                        "show Brillig registers (valid when executing a Brillig block)",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::ShowBrilligRegisters
                            )
                        )
                    ))
                    .add("memory", Command::new(
                        "show Brillig memory (valid when executing a Brillig block)",
                        vec![],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::ShowBrilligMemory
                            )
                        )
                    ))
                    .add("witness", Command::new(
                        "display a single witness from the witness map",
                        vec![CommandArgInfo::new_with_name(CommandArgType::I32, "index")],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::DisplaySingleWitness
                            )
                        )
                    ))
                    .add("witness", Command::new(
                        "update a witness with the given value",
                        vec![
                            CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                            CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                        ],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::UpdateWitness,
                            )
                        )
                    ))
                    .add("regset", Command::new(
                        "update a Brillig register with the given value",
                        vec![
                            CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                            CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                        ],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::RegSet,
                            )
                        )
                    ))
                    .add("memset", Command::new(
                        "update a Brillig memory cell with the given value",
                        vec![
                            CommandArgInfo::new_with_name(CommandArgType::I32, "index"),
                            CommandArgInfo::new_with_name(CommandArgType::String, "value"),
                        ],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::MemSet,
                            )
                        )
                    ))
                    .add("break", Command::new(
                        "add a breakpoint at an opcode location",
                        vec![
                            CommandArgInfo::new_with_name(CommandArgType::Custom, "location"),
                        ],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::BreakpointSet,
                            )
                        )
                    ))
                    .add("delete", Command::new(
                        "delete breakpoint at an opcode location",
                        vec![
                            CommandArgInfo::new_with_name(CommandArgType::Custom, "location"),
                        ],
                        Box::new(
                            DebuggerCommandHandler::new(
                                to_debugger.clone(),
                                ReplDebuggerMessageType::BreakpointDelete,
                            )
                        )
                    ))
                    .build().expect("Debugger error");

                            let mut repl_status = LoopStatus::Continue;
                            while repl_status == LoopStatus::Continue {
                                repl_status = repl.next().await.expect("Debugger REPL error");

                                //TODO: we probably need to change mini_async_repl's
                                //blocking readline to avoid this
                                tokio::task::yield_now().await;
                            }

                            to_debugger.send(ReplDebuggerMessage::Finalize).await;
                        });

                    // The debugger observer thread monitors finalization
                    // of the debugger.
                    // When the debugger emits an Option<WitnessMap> it is
                    // bubbled so Nargo decides what to do with that result.
                    let debugger_observer = spawn_local(async move {
                        while let Some(debugger_msg) = receive_from_debugger.recv().await {
                            return debugger_msg;
                        }

                        return None;
                    });

                    debugger.await.unwrap();
                    repl.await.unwrap();

                    return match debugger_observer.await {
                        Ok(r) => Ok(r),
                        Err(_e) => Ok(None),
                    };
                })
                .await;

        return result;
    })
}
