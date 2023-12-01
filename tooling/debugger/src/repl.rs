use crate::context::{DebugCommandResult, DebugContext};

use acvm::acir::circuit::{Circuit, Opcode, OpcodeLocation};
use acvm::acir::native_types::{Witness, WitnessMap};
use acvm::{BlackBoxFunctionSolver, FieldElement};

use nargo::{artifacts::debug::DebugArtifact, ops::DefaultForeignCallExecutor, NargoError};

use mini_async_repl::{
    anyhow,
    CommandStatus,
    Repl,
    command::{
        Command,
        CommandArgInfo,
        ExecuteCommand,
        validate,
        lift_validation_err,
    },
};

use codespan_reporting::files::Files;
use noirc_errors::Location;

use owo_colors::OwoColorize;

use std::cell::RefCell;
use std::rc::Rc;
use std::ops::Range;
use std::pin::Pin;
use std::future::Future;
use std::sync::{Arc, Mutex};

pub struct ReplDebugger<'a, B>
where B: BlackBoxFunctionSolver {
    context: DebugContext<'a, B>,
    blackbox_solver: &'a B,
    circuit: &'a Circuit,
    debug_artifact: &'a DebugArtifact,
    initial_witness: WitnessMap,
    last_result: DebugCommandResult,
}

impl<'a, B> ReplDebugger<'a, B> 
where
    B: BlackBoxFunctionSolver,
{
    pub fn new(
        blackbox_solver: &'a B,
        circuit: &'a Circuit,
        debug_artifact: &'a DebugArtifact,
        initial_witness: WitnessMap,
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

    fn step_acir_opcode(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.step_acir_opcode();
            self.handle_debug_command_result(result);
        }
    }

    fn step_into_opcode(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.step_into_opcode();
            self.handle_debug_command_result(result);
        }
    }

    fn next(&mut self) {
        if self.validate_in_progress() {
            let result = self.context.next();
            self.handle_debug_command_result(result);
        }
    }

    fn cont(&mut self) {
        if self.validate_in_progress() {
            println!("(Continuing execution...)");
            let result = self.context.cont();
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

struct NextACIRCommandHandler<'a, B>
where B: BlackBoxFunctionSolver {
    context_arc: Arc<Mutex<Option<ReplDebugger<'a, B>>>>,
}
impl<'a, B> NextACIRCommandHandler<'a, B>
where
    B: BlackBoxFunctionSolver
{
    pub fn new(context_arc: Arc<Mutex<Option<ReplDebugger<'a, B>>>>) -> Self
    where B: BlackBoxFunctionSolver
    {
        Self { context_arc }
    }
    async fn handle_command(&mut self) -> anyhow::Result<CommandStatus> {
        {
            let mut debugger_context = self.context_arc.lock().unwrap();
            if let Some(debugger_context) = debugger_context.as_mut() {
                debugger_context.step_acir_opcode();    
            }
            
            anyhow::Ok(CommandStatus::Done)
        }
    }
}
impl<'a, B> ExecuteCommand for NextACIRCommandHandler<'a, B>
where
    B: BlackBoxFunctionSolver
{
    fn execute(
        &mut self,
        args: Vec<String>,
        args_info: Vec<CommandArgInfo>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CommandStatus>> + '_>> {
        let valid = validate(args.clone(), args_info.clone());
        if let Err(e) = valid {
            return Box::pin(lift_validation_err(Err(e)));
        }
        Box::pin(self.handle_command())
    }
}

pub async fn run<'a, B: BlackBoxFunctionSolver>(
    blackbox_solver: &'a B,
    circuit: &'a Circuit,
    debug_artifact: &'a DebugArtifact,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    // let context =
    // RefCell::new(ReplDebugger::new(blackbox_solver, circuit, debug_artifact, initial_witness));
    let repl_debugger = ReplDebugger::new(blackbox_solver, circuit, debug_artifact, initial_witness);
    let debugger_arc = Arc::new(Mutex::new(Some(repl_debugger)));

    // let context_rc = Rc::new(context);

    {
        let mut debugger_context = debugger_arc.lock().unwrap();
        if let Some(debugger_context) = debugger_context.as_mut() {
            debugger_context.show_current_vm_status();
        }
    }
    
    let mut repl = Repl::builder()
        .add(
            "step", Command::new(
                "step to the next ACIR opcode",
                vec![],
                Box::new(NextACIRCommandHandler::new(debugger_arc.clone()))
            )
        )
        .build()
        .expect("Failed to initialize debugger repl");

    repl.run().await.expect("Debugger error");

    // REPL execution has finished.
    // Drop it so that we can move fields out from `context` again.
    drop(repl);

    let mut debugger_context = debugger_arc.lock().unwrap();
    let moved_out_context = std::mem::replace(&mut *debugger_context, None);

    if let Some(moved_out_context) = moved_out_context {
        if moved_out_context.is_solved() {
            let solved_witness = moved_out_context.finalize();
            Ok(Some(solved_witness))
        } else {
            Ok(None)
        }
    } else {
        //TODO: how do we handle this "could not get lock" case?
        Ok(None)
    }
}
