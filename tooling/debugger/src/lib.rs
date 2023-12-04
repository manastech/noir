mod context;
mod repl;

use acvm::BlackBoxFunctionSolver;
use acvm::{acir::circuit::Circuit, acir::native_types::WitnessMap};

use nargo::artifacts::debug::DebugArtifact;

use nargo::NargoError;

use std::sync::Arc;

use crate::repl::ReplDebuggerInput;

pub async fn debug_circuit<B: BlackBoxFunctionSolver>(
    input: Arc<ReplDebuggerInput<B>>,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    repl::run(input, initial_witness).await
}
