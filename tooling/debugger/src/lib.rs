mod context;
mod dap;
mod debug;
pub mod errors;
mod foreign_calls;
mod repl;
mod source_code_printer;

use std::io::{Read, Write};

use ::dap::errors::ServerError;
use ::dap::server::Server;
pub use context::DebugExecutionResult;
pub use context::Project;

pub fn run_repl_session(
    project: Project,
    raw_source_printing: bool,
    foreign_call_resolver_url: Option<String>,
    pedantic_solving: bool,
) -> DebugExecutionResult {
    repl::run(project, raw_source_printing, foreign_call_resolver_url, pedantic_solving)
}

pub fn run_dap_loop<R: Read, W: Write>(
    server: &mut Server<R, W>,
    project: Project,
    pedantic_solving: bool,
    foreign_call_resolver_url: Option<String>,
) -> Result<DebugExecutionResult, ServerError> {
    dap::run_session(server, project, pedantic_solving, foreign_call_resolver_url)
}
