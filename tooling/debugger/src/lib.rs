use acvm::acir::circuit::OpcodeLocation;
use acvm::pwg::{ACVMStatus, ErrorLocation, OpcodeResolutionError, ACVM};
use acvm::BlackBoxFunctionSolver;
use acvm::{acir::circuit::Circuit, acir::native_types::WitnessMap};

use nargo::artifacts::debug::DebugArtifact;
use nargo::errors::ExecutionError;
use nargo::NargoError;

use nargo::ops::ForeignCallExecutor;

use thiserror::Error;

use easy_repl::{command, CommandStatus, Critical, Repl};
use std::cell::RefCell;

enum SolveResult {
    Done,
    Ok,
}

#[derive(Debug, Error)]
enum DebuggingError {
    /// ACIR circuit execution error
    #[error(transparent)]
    ExecutionError(#[from] nargo::errors::ExecutionError),

    /// Oracle handling error
    #[error(transparent)]
    ForeignCallError(#[from] noirc_printable_type::ForeignCallError),
}

struct DebugContext<'backend, B: BlackBoxFunctionSolver> {
    acvm: Option<ACVM<'backend, B>>,
    debug_artifact: DebugArtifact,
    foreign_call_executor: ForeignCallExecutor,
    circuit: Circuit,
    show_output: bool,
}

impl<'backend, B: BlackBoxFunctionSolver> DebugContext<'backend, B> {
    fn step_opcode(&mut self) -> Result<SolveResult, DebuggingError> {
        // Assert messages are not a map due to https://github.com/noir-lang/acvm/issues/522
        let assert_messages = &self.circuit.assert_messages;
        let get_assert_message = |opcode_location| {
            assert_messages
                .iter()
                .find(|(loc, _)| loc == opcode_location)
                .map(|(_, message)| message.clone())
        };

        let solver_status = self.acvm.as_mut().unwrap().solve_opcode();

        match solver_status {
            ACVMStatus::Solved => Ok(SolveResult::Done),
            ACVMStatus::InProgress => Ok(SolveResult::Ok),
            ACVMStatus::Failure(error) => {
                let call_stack = match &error {
                    OpcodeResolutionError::UnsatisfiedConstrain {
                        opcode_location: ErrorLocation::Resolved(opcode_location),
                    } => Some(vec![*opcode_location]),
                    OpcodeResolutionError::BrilligFunctionFailed { call_stack, .. } => {
                        Some(call_stack.clone())
                    }
                    _ => None,
                };

                Err(DebuggingError::ExecutionError(match call_stack {
                    Some(call_stack) => {
                        if let Some(assert_message) = get_assert_message(
                            call_stack.last().expect("Call stacks should not be empty"),
                        ) {
                            ExecutionError::AssertionFailed(assert_message, call_stack)
                        } else {
                            ExecutionError::SolvingError(error)
                        }
                    }
                    None => ExecutionError::SolvingError(error),
                }))
            }
            ACVMStatus::RequiresForeignCall(foreign_call) => {
                let foreign_call_result =
                    self.foreign_call_executor.execute(&foreign_call, self.show_output)?;
                self.acvm.as_mut().unwrap().resolve_pending_foreign_call(foreign_call_result);
                Ok(SolveResult::Ok)
            }
        }
    }

    fn show_current_vm_status(&self) {
        let ip = self.acvm.as_ref().unwrap().instruction_pointer();
        println!("Stopped at opcode {}: {}", ip, self.acvm.as_ref().unwrap().opcodes()[ip]);
        Self::show_source_code_location(&OpcodeLocation::Acir(ip), &self.debug_artifact);
    }

    fn show_source_code_location(location: &OpcodeLocation, debug_artifact: &DebugArtifact) {
        let locations = debug_artifact.debug_symbols[0].opcode_location(&location);
        match locations {
            Some(locations) => {
                for loc in locations {
                    let file = &debug_artifact.file_map[&loc.file];
                    let source = &file.source.as_str();
                    let start = loc.span.start() as usize;
                    let end = loc.span.end() as usize;
                    println!("At {}:{start}-{end}", file.path.as_path().display());
                    println!("\n{}\n", &source[start..end]);
                }
            }
            None => {}
        }
    }

    fn cont(&mut self) -> Result<SolveResult, DebuggingError> {
        loop {
            match self.step_opcode()? {
                SolveResult::Done => break,
                SolveResult::Ok => {}
            }
        }
        Ok(SolveResult::Done)
    }
}

impl From<nargo::errors::NargoError> for DebuggingError {
    fn from(e: nargo::errors::NargoError) -> Self {
        match e {
            NargoError::ForeignCallError(e1) => DebuggingError::ForeignCallError(e1),
            _ => DebuggingError::ExecutionError(ExecutionError::Halted),
        }
    }
}

fn map_command_status(result: SolveResult) -> CommandStatus {
    match result {
        SolveResult::Ok => CommandStatus::Done,
        SolveResult::Done => CommandStatus::Quit,
    }
}

pub fn debug_circuit<B: BlackBoxFunctionSolver>(
    blackbox_solver: &B,
    circuit: Circuit,
    debug_artifact: DebugArtifact,
    initial_witness: WitnessMap,
    show_output: bool,
) -> Result<WitnessMap, NargoError> {
    let opcodes = circuit.opcodes.clone();

    let context = RefCell::new(DebugContext {
        acvm: Some(ACVM::new(blackbox_solver, opcodes, initial_witness)),
        foreign_call_executor: ForeignCallExecutor::default(),
        circuit,
        debug_artifact,
        show_output,
    });
    let ref_step = &context;
    let ref_cont = &context;

    let solved = RefCell::new(false);

    context.borrow().show_current_vm_status();

    let handle_result = |result| {
        solved.replace(match result {
            SolveResult::Done => true,
            _ => false,
        });
        Ok(map_command_status(result))
    };

    let mut repl = Repl::builder()
        .add(
            "s",
            command! {
                "step to the next opcode",
                () => || {
                    let result = ref_step.borrow_mut().step_opcode().into_critical()?;
                    ref_step.borrow().show_current_vm_status();
                    handle_result(result)
                }
            },
        )
        .add(
            "c",
            command! {
                "continue execution until the end of the program",
                () => || {
                    println!("(Continuing execution...)");
                    let result = ref_cont.borrow_mut().cont().into_critical()?;
                    handle_result(result)
                }
            },
        )
        .build()
        .expect("Failed to initialize debugger repl");

    repl.run().expect("Debugger error");

    let acvm = context.borrow_mut().acvm.take().unwrap();
    if *solved.borrow() {
        let solved_witness = acvm.finalize();
        Ok(solved_witness)
    } else {
        Err(NargoError::ExecutionError(ExecutionError::Halted))
    }
}