use acvm::{
    acir::{
        brillig::BitSize,
        circuit::{brillig::BrilligBytecode, Circuit, Opcode},
        native_types::{Witness, WitnessMap, WitnessStack},
    },
    brillig_vm::MemoryValue,
    FieldElement,
};
use bn254_blackbox_solver::Bn254BlackBoxSolver;
use nargo::errors::Location;
use noirc_artifacts::debug::DebugArtifact;
use std::sync::mpsc::{Receiver, Sender};

use crate::{
    context::{DebugCommandResult, DebugContext, DebugLocation, DebugStackFrame},
    foreign_calls::DebugForeignCallExecutor,
};

// TODO: revisit this mod name

#[derive(Debug)]
pub(super) enum DebugCommandAPIResult {
    DebugCommandResult(DebugCommandResult),
    DebugLocation(Option<DebugLocation>),
    Opcodes(Vec<Opcode<FieldElement>>),
    Locations(Vec<Location>),
    DebugLocations(Vec<DebugLocation>),
    Bool(bool),
    WitnessMap(WitnessMap<FieldElement>),
    MemoryValue(Option<Vec<MemoryValue<FieldElement>>>),
    Unit(()),
    Variables(Vec<DebugStackFrame<FieldElement>>),
    WitnessStack(WitnessStack<FieldElement>),
    Field(Option<FieldElement>),
}

#[derive(Debug)]
pub(super) enum DebugCommandAPI {
    GetCurrentDebugLocation,
    GetOpcodes,
    GetOpcodesOfCircuit(u32),
    GetSourceLocationForDebugLocation(DebugLocation),
    GetCallStack,
    IsBreakpointSet(DebugLocation),
    IsValidDebugLocation(DebugLocation),
    AddBreakpoint(DebugLocation),
    DeleteBreakpoint(DebugLocation),
    GetWitnessMap,
    IsExecutingBrillig,
    GetBrilligMemory,
    WriteBrilligMemory(usize, FieldElement, BitSize),
    OverwriteWitness(Witness, FieldElement),
    GetVariables,
    IsSolved,
    Restart,
    Finalize,
    FindOpcodeAtCurrentFileLine(i64),
    // execution control
    StepAcirOpcode,
    StepIntoOpcode,
    NextInto,
    NextOver,
    NextOut,
    Cont,
}

pub(super) fn start_debugger<'a>(
    command_rx: Receiver<DebugCommandAPI>,
    result_tx: Sender<DebugCommandAPIResult>,
    circuits: Vec<Circuit<FieldElement>>,
    debug_artifact: &'a DebugArtifact,
    initial_witness: WitnessMap<FieldElement>,
    foreign_call_executor: Box<dyn DebugForeignCallExecutor + 'a>,
    unconstrained_functions: Vec<BrilligBytecode<FieldElement>>,
    pedantic_solving: bool,
) {
    let blackbox_solver = Bn254BlackBoxSolver(pedantic_solving);
    let mut context = DebugContext::new(
        &blackbox_solver,
        &circuits,
        debug_artifact,
        initial_witness.clone(),
        foreign_call_executor,
        &unconstrained_functions,
    );

    println!("Debugger ready for receiving messages..");
    loop {
        // recv blocks until it receives message
        if let Ok(received) = command_rx.recv() {
            let result = match received {
                DebugCommandAPI::GetCurrentDebugLocation => {
                    DebugCommandAPIResult::DebugLocation(context.get_current_debug_location())
                }
                DebugCommandAPI::GetOpcodes => {
                    DebugCommandAPIResult::Opcodes(context.get_opcodes().to_owned())
                }
                DebugCommandAPI::GetOpcodesOfCircuit(circuit_id) => DebugCommandAPIResult::Opcodes(
                    context.get_opcodes_of_circuit(circuit_id).to_owned(),
                ),
                DebugCommandAPI::GetSourceLocationForDebugLocation(debug_location) => {
                    DebugCommandAPIResult::Locations(
                        context.get_source_location_for_debug_location(&debug_location),
                    )
                }
                DebugCommandAPI::GetCallStack => {
                    DebugCommandAPIResult::DebugLocations(context.get_call_stack())
                }
                DebugCommandAPI::IsBreakpointSet(debug_location) => {
                    DebugCommandAPIResult::Bool(context.is_breakpoint_set(&debug_location))
                }
                DebugCommandAPI::IsValidDebugLocation(debug_location) => {
                    DebugCommandAPIResult::Bool(context.is_valid_debug_location(&debug_location))
                }
                DebugCommandAPI::AddBreakpoint(debug_location) => {
                    DebugCommandAPIResult::Bool(context.add_breakpoint(debug_location))
                }
                DebugCommandAPI::DeleteBreakpoint(debug_location) => {
                    DebugCommandAPIResult::Bool(context.delete_breakpoint(&debug_location))
                }
                DebugCommandAPI::Restart => {
                    context.restart();
                    DebugCommandAPIResult::Unit(())
                }
                DebugCommandAPI::GetWitnessMap => {
                    DebugCommandAPIResult::WitnessMap(context.get_witness_map().clone())
                }
                DebugCommandAPI::IsExecutingBrillig => {
                    DebugCommandAPIResult::Bool(context.is_executing_brillig())
                }
                DebugCommandAPI::GetBrilligMemory => DebugCommandAPIResult::MemoryValue(
                    context.get_brillig_memory().map(|values| values.to_vec()),
                ),
                DebugCommandAPI::WriteBrilligMemory(ptr, value, bit_size) => {
                    context.write_brillig_memory(ptr, value, bit_size);
                    DebugCommandAPIResult::Unit(())
                }
                DebugCommandAPI::OverwriteWitness(witness, value) => {
                    DebugCommandAPIResult::Field(context.overwrite_witness(witness, value))
                }

                DebugCommandAPI::GetVariables => DebugCommandAPIResult::Variables(
                    context.get_variables().iter().map(DebugStackFrame::from).collect(),
                ),
                DebugCommandAPI::IsSolved => DebugCommandAPIResult::Bool(context.is_solved()),
                DebugCommandAPI::StepAcirOpcode => {
                    DebugCommandAPIResult::DebugCommandResult(context.step_acir_opcode())
                }
                DebugCommandAPI::StepIntoOpcode => {
                    DebugCommandAPIResult::DebugCommandResult(context.step_into_opcode())
                }
                DebugCommandAPI::NextInto => {
                    DebugCommandAPIResult::DebugCommandResult(context.next_into())
                }
                DebugCommandAPI::NextOver => {
                    DebugCommandAPIResult::DebugCommandResult(context.next_over())
                }
                DebugCommandAPI::NextOut => {
                    DebugCommandAPIResult::DebugCommandResult(context.next_out())
                }
                DebugCommandAPI::Cont => DebugCommandAPIResult::DebugCommandResult(context.cont()),
                DebugCommandAPI::FindOpcodeAtCurrentFileLine(line) => {
                    DebugCommandAPIResult::DebugLocation(
                        context.find_opcode_at_current_file_line(line),
                    )
                }
                DebugCommandAPI::Finalize => {
                    let witness_stack = context.finalize();
                    let _ = result_tx.send(DebugCommandAPIResult::WitnessStack(witness_stack));
                    // We need to stop the 'event loop' since `finalize` consumes the context
                    drop(result_tx);
                    drop(command_rx);
                    break;
                }
            };
            let Ok(()) = result_tx.send(result) else {
                drop(result_tx);
                drop(command_rx);
                break;
            };
        } else {
            println!("Upstream channel closed. Terminating debugger");
            drop(result_tx);
            drop(command_rx);
            break;
        }
    }
}
