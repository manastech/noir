pub mod context;

#[cfg(not(target_arch = "wasm32"))]
mod repl;

use acvm::BlackBoxFunctionSolver;
use acvm::{acir::circuit::Circuit, acir::native_types::WitnessMap};

use nargo::artifacts::debug::DebugArtifact;

use nargo::NargoError;

#[cfg(not(target_arch = "wasm32"))]
pub fn debug_circuit<B: BlackBoxFunctionSolver>(
    blackbox_solver: &B,
    circuit: &Circuit,
    debug_artifact: DebugArtifact,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    repl::run(blackbox_solver, circuit, &debug_artifact, initial_witness)
}

pub fn debug_echo(say: String) -> String {
    say
}
