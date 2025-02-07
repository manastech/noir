pub use self::check::check_program;
pub use self::compile::{
    collect_errors, compile_contract, compile_program, compile_program_with_debug_instrumenter,
    compile_workspace, report_errors,
};
pub use self::optimize::{optimize_contract, optimize_program};
pub use self::transform::{transform_contract, transform_program};

pub use self::execute::{execute_program, execute_program_with_profiling};
pub use self::test::{
    check_expected_failure_message, run_test, test_status_program_compile_fail,
    test_status_program_compile_pass, TestStatus,
};

mod check;
mod compile;
mod execute;
mod optimize;
mod test;
mod transform;
