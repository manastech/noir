mod context;
mod dap;
mod debug;
pub mod errors;
mod foreign_calls;
mod repl;
mod source_code_printer;

use std::io::{Read, Write};
use std::path::PathBuf;

use ::dap::errors::ServerError;
use ::dap::server::Server;
use acvm::acir::native_types::WitnessMap;
use acvm::FieldElement;
pub use context::DebugExecutionResult;

use noirc_driver::CompiledProgram;

pub fn run_repl_session(
    program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
    raw_source_printing: bool,
    foreign_call_resolver_url: Option<String>,
    root_path: PathBuf,
    package_name: String,
    pedantic_solving: bool,
) -> DebugExecutionResult {
    repl::run(
        program,
        initial_witness,
        raw_source_printing,
        foreign_call_resolver_url,
        root_path,
        package_name,
        pedantic_solving,
    )
}

pub fn run_dap_loop<R: Read, W: Write>(
    server: &mut Server<R, W>,
    program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
    root_path: PathBuf,
    package_name: String,
    pedantic_solving: bool,
    foreign_call_resolver_url: Option<String>,
) -> Result<DebugExecutionResult, ServerError> {
    dap::run_session(
        server,
        program,
        initial_witness,
        root_path,
        package_name,
        pedantic_solving,
        foreign_call_resolver_url,
    )
}
