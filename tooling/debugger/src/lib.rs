mod context;
mod repl;

use acvm::acir::native_types::WitnessMap;
use nargo::NargoError;
use noirc_driver::CompiledProgram;

pub fn debug_circuit(
    compiled_program: &CompiledProgram,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    repl::run(compiled_program, initial_witness)
}
