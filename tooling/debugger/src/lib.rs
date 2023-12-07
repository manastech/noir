mod context;
mod repl;

use acvm::acir::native_types::WitnessMap;
use nargo::NargoError;
use noirc_driver::CompiledProgram;

pub fn debug_with_repl(
    program: &CompiledProgram,
    initial_witness: WitnessMap,
) -> Result<Option<WitnessMap>, NargoError> {
    repl::run(program, initial_witness)
}
