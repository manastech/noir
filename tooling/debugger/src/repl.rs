use crate::context::{
    start_debugger, DebugCommandAPI, DebugCommandAPIResult, DebugCommandResult, DebugLocation,
    DebugStackFrame,
};

use acvm::AcirField;
use acvm::acir::brillig::BitSize;
use acvm::acir::circuit::brillig::{BrilligBytecode, BrilligFunctionId};
use acvm::acir::circuit::{Circuit, Opcode, OpcodeLocation};
use acvm::acir::native_types::{Witness, WitnessMap, WitnessStack};
use acvm::brillig_vm::MemoryValue;
use acvm::brillig_vm::brillig::Opcode as BrilligOpcode;
use acvm::FieldElement;
use nargo::{NargoError, PrintOutput};
use noirc_driver::CompiledProgram;

use crate::foreign_calls::DefaultDebugForeignCallExecutor;
use noirc_artifacts::debug::DebugArtifact;

use easy_repl::{CommandStatus, Repl, command};
use noirc_printable_type::PrintableValueDisplay;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::source_code_printer::print_source_code_location;

pub struct ReplDebugger<'a> {
    // context: DebugContext<'a, B>,
    command_sender: Sender<DebugCommandAPI>,
    result_receiver: Receiver<DebugCommandAPIResult>,
    // blackbox_solver: &'a B,
    debug_artifact: &'a DebugArtifact,
    // initial_witness: WitnessMap<FieldElement>,
    last_result: DebugCommandResult,

    // ACIR functions to debug
    circuits: &'a [Circuit<FieldElement>],

    // Brillig functions referenced from the ACIR circuits above
    unconstrained_functions: &'a [BrilligBytecode<FieldElement>],

    // whether to print the source without highlighting, pretty-printing,
    // or line numbers
    raw_source_printing: bool,
}

impl<'a> ReplDebugger<'a> {
    pub fn new(
        circuits: &'a [Circuit<FieldElement>],
        debug_artifact: &'a DebugArtifact,
        unconstrained_functions: &'a [BrilligBytecode<FieldElement>],
        raw_source_printing: bool,
        command_sender: Sender<DebugCommandAPI>,
        result_receiver: Receiver<DebugCommandAPIResult>,
    ) -> Self {
        let last_result = DebugCommandResult::Ok; // TODO: handle circuit with no opcodes

        Self {
            command_sender,
            result_receiver,
            circuits,
            debug_artifact,
            last_result,
            unconstrained_functions,
            raw_source_printing,
        }
    }

    // FIXME: this probably reads better with match expression
    fn call_debugger(&self, command: DebugCommandAPI) -> DebugCommandAPIResult {
        if let Ok(()) = self.command_sender.send(command) {
            let Ok(result) = self.result_receiver.recv() else {
                panic!("Debugger closed connection unexpectedly");
            };
            result
        } else {
            panic!("Could not communicate with debugger")
        }
    }

    pub fn show_current_vm_status(&self) {
        let location = self.get_current_debug_location();

        match location {
            None => println!("Finished execution"),
            Some(location) => {
                let circuit_id = location.circuit_id;
                let opcodes = self.get_opcodes_of_circuit(circuit_id);

                match &location.opcode_location {
                    OpcodeLocation::Acir(ip) => {
                        println!("At opcode {} :: {}", location, opcodes[*ip]);
                    }
                    OpcodeLocation::Brillig { acir_index, brillig_index } => {
                        let brillig_bytecode =
                            if let Opcode::BrilligCall { id, .. } = opcodes[*acir_index] {
                                &self.unconstrained_functions[id.as_usize()].bytecode
                            } else {
                                unreachable!("Brillig location does not contain Brillig opcodes");
                            };
                        println!(
                            "At opcode {} :: {:?}",
                            location, brillig_bytecode[*brillig_index]
                        );
                    }
                }
                let result = self
                    .call_debugger(DebugCommandAPI::GetSourceLocationForDebugLocation(location));
                let DebugCommandAPIResult::Locations(locations) = result else {
                    panic!("Unwanted result")
                };

                print_source_code_location(
                    self.debug_artifact,
                    &locations,
                    self.raw_source_printing,
                );
            }
        }
    }

    fn send_execution_control_command(&self, command: DebugCommandAPI) -> DebugCommandResult {
        let result = self.call_debugger(command);
        let DebugCommandAPIResult::DebugCommandResult(command_result) = result else {
            panic!("Unwanted result")
        };
        command_result
    }

    // TODO: find a better name
    fn send_bool_command(&self, command: DebugCommandAPI) -> bool {
        let result = self.call_debugger(command);
        let DebugCommandAPIResult::Bool(status) = result else { panic!("Unwanted result") };
        status
    }

    fn get_opcodes_of_circuit(&self, circuit_id: u32) -> Vec<Opcode<FieldElement>> {
        let result = self.call_debugger(DebugCommandAPI::GetOpcodesOfCircuit(circuit_id));
        let DebugCommandAPIResult::Opcodes(opcodes) = result else { panic!("Unwanted result") };
        opcodes
    }

    fn get_current_debug_location(&self) -> Option<DebugLocation> {
        let result = self.call_debugger(DebugCommandAPI::GetCurrentDebugLocation);
        let DebugCommandAPIResult::DebugLocation(location) = result else {
            panic!("Unwanted result")
        };
        location
    }

    fn is_breakpoint_set(&self, debug_location: DebugLocation) -> bool {
        self.send_bool_command(DebugCommandAPI::IsBreakpointSet(debug_location))
    }

    fn is_valid_debug_location(&self, debug_location: DebugLocation) -> bool {
        self.send_bool_command(DebugCommandAPI::IsValidDebugLocation(debug_location))
    }

    fn add_breakpoint(&self, debug_location: DebugLocation) -> bool {
        self.send_bool_command(DebugCommandAPI::AddBreakpoint(debug_location))
    }

    fn delete_breakpoint(&self, debug_location: DebugLocation) -> bool {
        self.send_bool_command(DebugCommandAPI::DeleteBreakpoint(debug_location))
    }

    fn is_executing_brillig(&self) -> bool {
        self.send_bool_command(DebugCommandAPI::IsExecutingBrillig)
    }
    fn get_brillig_memory(&self) -> Option<Vec<MemoryValue<FieldElement>>> {
        let result = self.call_debugger(DebugCommandAPI::GetBrilligMemory);
        let DebugCommandAPIResult::MemoryValue(mem) = result else { panic!("Unwanted result") };
        mem
    }
    fn get_variables(&self) -> Vec<DebugStackFrame<FieldElement>> {
        let result = self.call_debugger(DebugCommandAPI::GetVariables);
        let DebugCommandAPIResult::Variables(vars) = result else { panic!("Unwanted result") };
        vars
    }

    fn overwrite_witness(&self, witness: Witness, value: FieldElement) -> Option<FieldElement> {
        let result = self.call_debugger(DebugCommandAPI::OverwriteWitness(witness, value));
        let DebugCommandAPIResult::Field(field) = result else { panic!("Unwanted result") };
        field
    }

    fn is_solved(&self) -> bool {
        self.send_bool_command(DebugCommandAPI::IsSolved)
    }

    fn restart_debugger(&self) {
        let result = self.call_debugger(DebugCommandAPI::Restart);
        let DebugCommandAPIResult::Unit = result else { panic!("Unwanted result") };
    }

    fn get_witness_map(&self) -> WitnessMap<FieldElement> {
        let result = self.call_debugger(DebugCommandAPI::GetWitnessMap);
        let DebugCommandAPIResult::WitnessMap(witness_map) = result else {
            panic!("Unwanted result")
        };
        witness_map
    }
    fn find_opcode_at_current_file_line(&self, line_number: i64) -> Option<DebugLocation> {
        let result = self.call_debugger(DebugCommandAPI::FindOpcodeAtCurrentFileLine(line_number));
        let DebugCommandAPIResult::DebugLocation(location) = result else {
            panic!("Unwanted result")
        };
        location
    }
    fn finalize(self) -> WitnessStack<FieldElement> {
        let result = self.call_debugger(DebugCommandAPI::Finalize);
        let DebugCommandAPIResult::WitnessStack(stack) = result else { panic!("Unwanted result") };
        stack
    }

    fn show_stack_frame(&self, index: usize, debug_location: &DebugLocation) {
        let result = self.call_debugger(DebugCommandAPI::GetOpcodes);
        let DebugCommandAPIResult::Opcodes(opcodes) = result else { panic!("Unwanted result") };
        match &debug_location.opcode_location {
            OpcodeLocation::Acir(instruction_pointer) => {
                println!(
                    "Frame #{index}, opcode {} :: {}",
                    debug_location, opcodes[*instruction_pointer]
                )
            }
            OpcodeLocation::Brillig { acir_index, brillig_index } => {
                let brillig_bytecode = if let Opcode::BrilligCall { id, .. } = opcodes[*acir_index]
                {
                    &self.unconstrained_functions[id.as_usize()].bytecode
                } else {
                    unreachable!("Brillig location does not contain Brillig opcodes");
                };
                println!(
                    "Frame #{index}, opcode {} :: {:?}",
                    debug_location, brillig_bytecode[*brillig_index]
                );
            }
        }
        // todo: should we clone the debug_location so it can be sent?
        let result =
            self.call_debugger(DebugCommandAPI::GetSourceLocationForDebugLocation(*debug_location));
        let DebugCommandAPIResult::Locations(locations) = result else { panic!("Unwanted result") };

        print_source_code_location(self.debug_artifact, &locations, self.raw_source_printing);
    }

    pub fn show_current_call_stack(&self) {
        // let call_stack = self.context.get_ca
        let result = self.call_debugger(DebugCommandAPI::GetCallStack);
        let DebugCommandAPIResult::DebugLocations(call_stack) = result else {
            panic!("Unwanted result")
        };

        if call_stack.is_empty() {
            println!("Finished execution. Call stack empty.");
            return;
        }

        for (i, frame_location) in call_stack.iter().enumerate() {
            self.show_stack_frame(i, frame_location);
        }
    }

    fn display_opcodes(&self) {
        for i in 0..self.circuits.len() {
            self.display_opcodes_of_circuit(i as u32);
        }
    }

    fn display_opcodes_of_circuit(&self, circuit_id: u32) {
        let current_opcode_location =
            self.get_current_debug_location().and_then(|debug_location| {
                if debug_location.circuit_id == circuit_id {
                    Some(debug_location.opcode_location)
                } else {
                    None
                }
            });
        let opcodes = self.get_opcodes_of_circuit(circuit_id);
        let current_acir_index: Option<usize> = match current_opcode_location {
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
            } else if self.is_breakpoint_set(DebugLocation {
                circuit_id,
                opcode_location: OpcodeLocation::Acir(acir_index),
                brillig_function_id: None,
            }) {
                " *"
            } else {
                ""
            }
        };
        let brillig_marker = |acir_index, brillig_index, brillig_function_id| {
            if current_acir_index == Some(acir_index) && brillig_index == current_brillig_index {
                "->"
            } else if self.is_breakpoint_set(DebugLocation {
                circuit_id,
                opcode_location: OpcodeLocation::Brillig { acir_index, brillig_index },
                brillig_function_id: Some(brillig_function_id),
            }) {
                " *"
            } else {
                ""
            }
        };
        let print_brillig_bytecode =
            |acir_index,
             bytecode: &[BrilligOpcode<FieldElement>],
             brillig_function_id: BrilligFunctionId| {
                for (brillig_index, brillig_opcode) in bytecode.iter().enumerate() {
                    println!(
                        "{:>2}:{:>3}.{:<2} |{:2} {:?}",
                        circuit_id,
                        acir_index,
                        brillig_index,
                        brillig_marker(acir_index, brillig_index, brillig_function_id),
                        brillig_opcode
                    );
                }
            };
        for (acir_index, opcode) in opcodes.iter().enumerate() {
            let marker = outer_marker(acir_index);
            match &opcode {
                Opcode::BrilligCall { id, inputs, outputs, .. } => {
                    println!(
                        "{:>2}:{:>3} {:2} BRILLIG CALL id={} inputs={:?}",
                        circuit_id, acir_index, marker, id, inputs
                    );
                    println!("          |       outputs={:?}", outputs);
                    let bytecode = &self.unconstrained_functions[id.as_usize()].bytecode;
                    print_brillig_bytecode(acir_index, bytecode, *id);
                }
                _ => println!("{:>2}:{:>3} {:2} {:?}", circuit_id, acir_index, marker, opcode),
            }
        }
    }

    fn add_breakpoint_at(&mut self, location: DebugLocation) {
        if !self.is_valid_debug_location(location) {
            println!("Invalid location {location}");
        } else if self.add_breakpoint(location) {
            println!("Added breakpoint at {location}");
        } else {
            println!("Breakpoint at {location} already set");
        }
    }

    fn add_breakpoint_at_line(&mut self, line_number: i64) {

        let best_location =
            self.find_opcode_at_current_file_line(line_number);

        match best_location {
            Some(location) => {
                println!("Added breakpoint at line {}", line_number);
                self.add_breakpoint_at(location)
            }
            None => println!("No opcode at line {}", line_number),
        }
    }

    fn delete_breakpoint_at(&mut self, location: DebugLocation) {
        if self.delete_breakpoint(location) {
            println!("Breakpoint at {location} deleted");
        } else {
            println!("Breakpoint at {location} not set");
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
            let result = self.send_execution_control_command(DebugCommandAPI::StepAcirOpcode);
            self.handle_debug_command_result(result);
        }
    }

    fn step_into_opcode(&mut self) {
        if self.validate_in_progress() {
            let result = self.send_execution_control_command(DebugCommandAPI::StepIntoOpcode);
            self.handle_debug_command_result(result);
        }
    }

    fn next_into(&mut self) {
        if self.validate_in_progress() {
            let result = self.send_execution_control_command(DebugCommandAPI::NextInto);
            self.handle_debug_command_result(result);
        }
    }

    fn next_over(&mut self) {
        if self.validate_in_progress() {
            let result = self.send_execution_control_command(DebugCommandAPI::NextOver);
            self.handle_debug_command_result(result);
        }
    }

    fn next_out(&mut self) {
        if self.validate_in_progress() {
            let result = self.send_execution_control_command(DebugCommandAPI::NextOut);
            self.handle_debug_command_result(result);
        }
    }

    fn cont(&mut self) {
        if self.validate_in_progress() {
            println!("(Continuing execution...)");
            let result = self.send_execution_control_command(DebugCommandAPI::Cont);
            self.handle_debug_command_result(result);
        }
    }

    fn restart_session(&mut self) {
        self.restart_debugger();
        self.last_result = DebugCommandResult::Ok;
        println!("Restarted debugging session.");
        self.show_current_vm_status();
    }

    pub fn show_witness_map(&self) {
        let witness_map = self.get_witness_map();
        // NOTE: we need to clone() here to get the iterator
        for (witness, value) in witness_map.clone().into_iter() {
            println!("_{} = {value}", witness.witness_index());
        }
    }

    pub fn show_witness(&self, index: u32) {
        if let Some(value) = self.get_witness_map().get_index(index) {
            println!("_{} = {value}", index);
        }
    }

    pub fn update_witness(&mut self, index: u32, value: String) {
        let Some(field_value) = FieldElement::try_from_str(&value) else {
            println!("Invalid witness value: {value}");
            return;
        };

        let witness = Witness::from(index);
        _ = self.overwrite_witness(witness, field_value);
        println!("_{} = {value}", index);
    }

    pub fn show_brillig_memory(&self) {
        if !self.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }

        let Some(memory) = self.get_brillig_memory() else {
            // this can happen when just entering the Brillig block since ACVM
            // would have not initialized the Brillig VM yet; in fact, the
            // Brillig code may be skipped altogether
            println!("Brillig VM memory not available");
            return;
        };

        for (index, value) in memory.iter().enumerate() {
            // Zero field is the default value, we omit it when printing memory
            if let MemoryValue::Field(field) = value {
                if field == &FieldElement::zero() {
                    continue;
                }
            }
            println!("{index} = {}", value);
        }
    }

    pub fn write_brillig_memory(&mut self, index: usize, value: String, bit_size: u32) {
        let Some(field_value) = FieldElement::try_from_str(&value) else {
            println!("Invalid value: {value}");
            return;
        };

        let Ok(bit_size) = BitSize::try_from_u32::<FieldElement>(bit_size) else {
            println!("Invalid bit size: {bit_size}");
            return;
        };

        if !self.is_executing_brillig() {
            println!("Not executing a Brillig block");
            return;
        }
        let result =
            self.call_debugger(DebugCommandAPI::WriteBrilligMemory(index, field_value, bit_size));
        let DebugCommandAPIResult::Unit = result else { panic!("Unwanted result") };
    }

    pub fn show_vars(&self) {
        for frame in self.get_variables() {
            println!("{}({})", frame.function_name, frame.function_params.join(", "));
            for (var_name, value, var_type) in frame.variables.iter() {
                let printable_value =
                    PrintableValueDisplay::Plain((*value).clone(), (*var_type).clone());
                println!("  {var_name}:{var_type:?} = {}", printable_value);
            }
        }
    }

    fn last_error(self) -> Option<NargoError<FieldElement>> {
        match self.last_result {
            DebugCommandResult::Error(error) => Some(error),
            _ => None,
        }
    }
}

pub fn run(
    program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
    raw_source_printing: bool,
    foreign_call_resolver_url: Option<String>,
    root_path: Option<PathBuf>,
    package_name: String,
    pedantic_solving: bool,
) -> Result<Option<WitnessStack<FieldElement>>, NargoError<FieldElement>> {
    let debugger_circuits = program.program.functions.clone();
    let circuits = &program.program.functions;
    let debugger_artifact =
        DebugArtifact { debug_symbols: program.debug.clone(), file_map: program.file_map.clone() };
    let debug_artifact = DebugArtifact { debug_symbols: program.debug, file_map: program.file_map };
    let debugger_unconstrained_functions = program.program.unconstrained_functions.clone();
    let unconstrained_functions = &program.program.unconstrained_functions;

    let foreign_call_executor = Box::new(DefaultDebugForeignCallExecutor::from_artifact(
        PrintOutput::Stdout,
        foreign_call_resolver_url,
        &debugger_artifact,
        root_path,
        package_name,
    ));

    let (command_tx, command_rx) = mpsc::channel::<DebugCommandAPI>();
    let (result_tx, result_rx) = mpsc::channel::<DebugCommandAPIResult>();
    thread::spawn(move || {
        start_debugger(
            command_rx,
            result_tx,
            debugger_circuits,
            &debugger_artifact,
            initial_witness,
            foreign_call_executor,
            debugger_unconstrained_functions,
            pedantic_solving,
        );
    });

    let context = RefCell::new(ReplDebugger::new(
        circuits,
        &debug_artifact,
        unconstrained_functions,
        raw_source_printing,
        command_tx,
        result_rx,
    ));
    let ref_context = &context;

    ref_context.borrow().show_current_vm_status();

    let mut repl = Repl::builder()
        .add(
            "step",
            command! {
                "step to the next ACIR opcode",
                () => || {
                    ref_context.borrow_mut().step_acir_opcode();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "into",
            command! {
                "step into to the next opcode",
                () => || {
                    ref_context.borrow_mut().step_into_opcode();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "next",
            command! {
                "step until a new source location is reached",
                () => || {
                    ref_context.borrow_mut().next_into();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "over",
            command! {
                "step until a new source location is reached without diving into function calls",
                () => || {
                    ref_context.borrow_mut().next_over();
                    Ok(CommandStatus::Done)
                }
            }
        )
        .add(
            "out",
            command! {
                "step until a new source location is reached and the current stack frame is finished",
                () => || {
                    ref_context.borrow_mut().next_out();
                    Ok(CommandStatus::Done)
                }
            }
        )
        .add(
            "continue",
            command! {
                "continue execution until the end of the program",
                () => || {
                    ref_context.borrow_mut().cont();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "restart",
            command! {
                "restart the debugging session",
                () => || {
                    ref_context.borrow_mut().restart_session();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "opcodes",
            command! {
                "display ACIR opcodes",
                () => || {
                    ref_context.borrow().display_opcodes();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "break",
            command! {
                "add a breakpoint at a line of the current file",
                (line_number: i64) => |line_number| {
                    ref_context.borrow_mut().add_breakpoint_at_line(line_number);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "break",
            command! {
                "add a breakpoint at an opcode location",
                (LOCATION:DebugLocation) => |location| {
                    ref_context.borrow_mut().add_breakpoint_at(location);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "delete",
            command! {
                "delete breakpoint at an opcode location",
                (LOCATION:DebugLocation) => |location| {
                    ref_context.borrow_mut().delete_breakpoint_at(location);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "witness",
            command! {
                "show witness map",
                () => || {
                    ref_context.borrow().show_witness_map();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "witness",
            command! {
                "display a single witness from the witness map",
                (index: u32) => |index| {
                    ref_context.borrow().show_witness(index);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "witness",
            command! {
                "update a witness with the given value",
                (index: u32, value: String) => |index, value| {
                    ref_context.borrow_mut().update_witness(index, value);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "memory",
            command! {
                "show Brillig memory (valid when executing a Brillig block)",
                () => || {
                    ref_context.borrow().show_brillig_memory();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "memset",
            command! {
                "update a Brillig memory cell with the given value",
                (index: usize, value: String, bit_size: u32) => |index, value, bit_size| {
                    ref_context.borrow_mut().write_brillig_memory(index, value, bit_size);
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "stacktrace",
            command! {
                "display the current stack trace",
                () => || {
                    ref_context.borrow().show_current_call_stack();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .add(
            "vars",
            command! {
                "show variables for each function scope available at this point in execution",
                () => || {
                    ref_context.borrow_mut().show_vars();
                    Ok(CommandStatus::Done)
                }
            },
        )
        .build()
        .expect("Failed to initialize debugger repl");

    repl.run().expect("Debugger error");
    // REPL execution has finished.
    // Drop it so that we can move fields out from `context` again.
    drop(repl);

    if context.borrow().is_solved() {
        let solved_witness_stack = context.into_inner().finalize();
        Ok(Some(solved_witness_stack))
    } else {
        match context.into_inner().last_error() {
            // Expose the last known error
            Some(error) => Err(error),
            None => Ok(None),
        }
    }
}
