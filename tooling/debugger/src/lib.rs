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
use acvm::acir::native_types::{WitnessMap, WitnessStack};
use acvm::{BlackBoxFunctionSolver, FieldElement};

use nargo::NargoError;
use noirc_driver::CompiledProgram;

pub fn run_repl_session(
    program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
    raw_source_printing: bool,
    foreign_call_resolver_url: Option<String>,
    root_path: Option<PathBuf>,
    package_name: String,
    pedantic_solving: bool,
) -> Result<Option<WitnessStack<FieldElement>>, NargoError<FieldElement>> {
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

pub fn run_dap_loop<R: Read, W: Write, B: BlackBoxFunctionSolver<FieldElement>>(
    server: Server<R, W>,
    solver: &B,
    program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
    root_path: Option<PathBuf>,
    package_name: String,
) -> Result<(), ServerError> {
    dap::run_session(server, solver, program, initial_witness, root_path, package_name)
}
