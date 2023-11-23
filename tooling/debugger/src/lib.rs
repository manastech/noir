use acvm::{
    acir::{
        circuit::Circuit,
        native_types::WitnessMap,
    },
    BlackBoxFunctionSolver,
};
use nargo::{
    artifacts::debug::DebugArtifact,
    NargoError,
};

pub mod context;

#[cfg(not(target_arch = "wasm32"))]
mod repl;

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
