use std::{io::Write, path::PathBuf};

use acvm::{BlackBoxFunctionSolver, FieldElement};
use bn254_blackbox_solver::Bn254BlackBoxSolver;
use clap::Args;
use fm::FileManager;
use nargo::{
    insert_all_files_for_workspace_into_file_manager, ops::test_status_program_compile_fail,
    ops::test_status_program_compile_pass, ops::TestStatus, package::Package, parse_all,
    prepare_package,
};
use nargo_toml::{get_package_manifest, resolve_workspace_from_toml, PackageSelection};
use noirc_driver::{
    check_crate, compile_no_check, file_manager_with_stdlib, CompileOptions,
    NOIR_ARTIFACT_VERSION_STRING,
};
use noirc_frontend::{
    graph::CrateName,
    hir::{def_map::TestFunction, Context, FunctionNameMatch, ParsedFiles},
};
use rayon::prelude::{IntoParallelIterator, ParallelBridge, ParallelIterator};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{cli::check_cmd::check_crate_and_report_errors, errors::CliError};

use super::{execution_helpers::prepare_package_for_debug, NargoConfig};

/// Run the tests for this program
#[derive(Debug, Clone, Args)]
#[clap(visible_alias = "t")]
pub(crate) struct TestCommand {
    /// If given, only tests with names containing this string will be run
    test_name: Option<String>,

    /// Display output of `println` statements
    #[arg(long)]
    show_output: bool,

    /// Only run tests that match exactly
    #[clap(long)]
    exact: bool,

    /// The name of the package to test
    #[clap(long, conflicts_with = "workspace")]
    package: Option<CrateName>,

    /// Test all packages in the workspace
    #[clap(long, conflicts_with = "package")]
    workspace: bool,

    /// Debug tests
    #[clap(long)]
    debug: bool,

    #[clap(flatten)]
    compile_options: CompileOptions,

    /// JSON RPC url to solve oracle calls
    #[clap(long)]
    oracle_resolver: Option<String>,
}

pub(crate) fn run(args: TestCommand, config: NargoConfig) -> Result<(), CliError> {
    let toml_path = get_package_manifest(&config.program_dir)?;
    let default_selection =
        if args.workspace { PackageSelection::All } else { PackageSelection::DefaultOrAll };
    let selection = args.package.map_or(default_selection, PackageSelection::Selected);
    let workspace = resolve_workspace_from_toml(
        &toml_path,
        selection,
        Some(NOIR_ARTIFACT_VERSION_STRING.to_string()),
    )?;
    let debug_mode = args.debug;

    let mut workspace_file_manager = file_manager_with_stdlib(&workspace.root_dir);
    insert_all_files_for_workspace_into_file_manager(&workspace, &mut workspace_file_manager);
    let parsed_files = parse_all(&workspace_file_manager);

    let pattern = match &args.test_name {
        Some(name) => {
            if args.exact {
                FunctionNameMatch::Exact(name)
            } else {
                FunctionNameMatch::Contains(name)
            }
        }
        None => FunctionNameMatch::Anything,
    };

    let test_reports: Vec<Vec<(String, TestStatus)>> = workspace
        .into_iter()
        .par_bridge()
        .map(|package| {
            let mut parsed_files = parsed_files.clone();
            run_tests::<Bn254BlackBoxSolver>(
                &workspace_file_manager,
                &mut parsed_files,
                package,
                pattern,
                args.show_output,
                args.oracle_resolver.as_deref(),
                &args.compile_options,
                debug_mode,
            )
        })
        .collect::<Result<_, _>>()?;
    let test_report: Vec<(String, TestStatus)> = test_reports.into_iter().flatten().collect();

    if test_report.is_empty() {
        match &pattern {
            FunctionNameMatch::Exact(pattern) => {
                return Err(CliError::Generic(
                    format!("Found 0 tests matching input '{pattern}'.",),
                ))
            }
            FunctionNameMatch::Contains(pattern) => {
                return Err(CliError::Generic(format!("Found 0 tests containing '{pattern}'.",)))
            }
            // If we are running all tests in a crate, having none is not an error
            FunctionNameMatch::Anything => {}
        };
    }

    if test_report.iter().any(|(_, status)| status.failed()) {
        Err(CliError::Generic(String::new()))
    } else {
        Ok(())
    }
}

fn run_tests<S: BlackBoxFunctionSolver<FieldElement> + Default>(
    file_manager: &FileManager,
    parsed_files: &mut ParsedFiles,
    package: &Package,
    fn_name: FunctionNameMatch,
    show_output: bool,
    foreign_call_resolver_url: Option<&str>,
    compile_options: &CompileOptions,
    debug_mode: bool,
) -> Result<Vec<(String, TestStatus)>, CliError> {
    let test_functions =
        get_tests_in_package(file_manager, parsed_files, package, fn_name, compile_options)?;

    let count_all = test_functions.len();

    let debug_mode = if debug_mode && count_all > 1 {
        println!(
            "[{}] Ignoring --debug flag since debugging multiple test is disabled",
            package.name
        );
        false
    } else {
        debug_mode
    };

    let plural = if count_all == 1 { "" } else { "s" };
    println!("[{}] Running {count_all} test function{plural}", package.name);

    let test_report: Vec<(String, TestStatus)> = test_functions
        .into_par_iter()
        .map(|test_name| {
            let mut parsed_files = parsed_files.clone();
            let status = run_test::<S>(
                file_manager,
                &mut parsed_files,
                package,
                &test_name,
                show_output,
                foreign_call_resolver_url,
                compile_options,
                debug_mode,
            );

            (test_name, status)
        })
        .collect();

    display_test_report(file_manager, package, compile_options, &test_report)?;
    Ok(test_report)
}

fn run_test<S: BlackBoxFunctionSolver<FieldElement> + Default>(
    file_manager: &FileManager,
    parsed_files: &mut ParsedFiles,
    package: &Package,
    fn_name: &str,
    show_output: bool,
    foreign_call_resolver_url: Option<&str>,
    compile_options: &CompileOptions,
    debug_mode: bool,
) -> TestStatus {
    // This is really hacky but we can't share `Context` or `S` across threads.
    // We then need to construct a separate copy for each test.

    let (mut context, crate_id) = if debug_mode {
        prepare_package_for_debug(file_manager, parsed_files, package)
    } else {
        prepare_package(file_manager, parsed_files, package)
    };

    check_crate(&mut context, crate_id, compile_options)
        .expect("Any errors should have occurred when collecting test functions");

    let test_functions = context
        .get_all_test_functions_in_crate_matching(&crate_id, FunctionNameMatch::Exact(fn_name));
    let (_, test_function) = test_functions.first().expect("Test function should exist");

    let blackbox_solver = S::default();

    let test_function_has_no_arguments = context
        .def_interner
        .function_meta(&test_function.get_id())
        .function_signature()
        .0
        .is_empty();

    if test_function_has_no_arguments {
        if debug_mode {
            debug_test(package, &mut context, test_function, compile_options)
        } else {
            nargo::ops::run_test(
                &blackbox_solver,
                &mut context,
                test_function,
                show_output,
                foreign_call_resolver_url,
                compile_options,
            )
        }
    } else {
        use noir_fuzzer::FuzzedExecutor;
        use proptest::test_runner::TestRunner;

        let compiled_program: Result<noirc_driver::CompiledProgram, noirc_driver::CompileError> =
            if debug_mode {
                compile_no_check_for_debug(&mut context, test_function, compile_options)
            } else {
                compile_no_check(&mut context, compile_options, test_function.get_id(), None, false)
            };
        match compiled_program {
            Ok(compiled_program) => {
                let runner = TestRunner::default();

                // TODO: Run debugger
                let fuzzer = FuzzedExecutor::new(compiled_program.into(), runner);

                let result = fuzzer.fuzz();
                if result.success {
                    TestStatus::Pass
                } else {
                    TestStatus::Fail {
                        message: result.reason.unwrap_or_default(),
                        error_diagnostic: None,
                    }
                }
            }
            Err(err) => TestStatus::CompileError(err.into()),
        }
    }
}

// This is a copy and modified version of nargo::ops::run_test
// This first iteration of the tester debugger will
//  - run the debugger with the test code
//  - once the debugger ends (the user quits) the test will be executed without the debugger
fn debug_test(
    package: &Package,
    context: &mut Context,
    test_function: &TestFunction,
    config: &CompileOptions,
) -> TestStatus {
    let compiled_program = compile_no_check_for_debug(context, test_function, config);

    match compiled_program {
        Ok(compiled_program) => {
            // Run the backend to ensure the PWG evaluates functions like std::hash::pedersen,
            // otherwise constraints involving these expressions will not error.
            let compiled_program = nargo::ops::transform_program(
                compiled_program,
                acvm::acir::circuit::ExpressionWidth::Bounded { width: 4 },
            ); // TODO: remove expression_with hardcoded value

            let abi = compiled_program.abi.clone();
            let debug = compiled_program.debug.clone();

            // Debug test
            let debug_result = super::debug_cmd::debug_program_async(
                package,
                compiled_program,
                "Prover.toml",
                &None,
                &PathBuf::new(),
            ); //FIXME:  hardcoded prover_name, witness_name, target_dir

            match debug_result {
                Ok(result) => test_status_program_compile_pass(test_function, abi, debug, result),
                Err(error) => TestStatus::Fail {
                    message: format!("Debugger failed: {}", error),
                    error_diagnostic: None,
                },
            }
        }
        Err(err) => test_status_program_compile_fail(err, test_function),
    }
}

fn compile_no_check_for_debug(
    context: &mut Context,
    test_function: &TestFunction,
    config: &CompileOptions,
) -> Result<noirc_driver::CompiledProgram, noirc_driver::CompileError> {
    let config = CompileOptions { instrument_debug: true, force_brillig: true, ..config.clone() };
    compile_no_check(context, &config, test_function.get_id(), None, false)
}

fn get_tests_in_package(
    file_manager: &FileManager,
    parsed_files: &ParsedFiles,
    package: &Package,
    fn_name: FunctionNameMatch,
    compile_options: &CompileOptions,
) -> Result<Vec<String>, CliError> {
    let (mut context, crate_id) = prepare_package(file_manager, parsed_files, package);
    check_crate_and_report_errors(&mut context, crate_id, compile_options)?;

    Ok(context
        .get_all_test_functions_in_crate_matching(&crate_id, fn_name)
        .into_iter()
        .map(|(test_name, _)| test_name)
        .collect())
}

fn display_test_report(
    file_manager: &FileManager,
    package: &Package,
    compile_options: &CompileOptions,
    test_report: &[(String, TestStatus)],
) -> Result<(), CliError> {
    let writer = StandardStream::stderr(ColorChoice::Always);
    let mut writer = writer.lock();

    for (test_name, test_status) in test_report {
        write!(writer, "[{}] Testing {test_name}... ", package.name)
            .expect("Failed to write to stderr");
        writer.flush().expect("Failed to flush writer");

        match &test_status {
            TestStatus::Pass { .. } => {
                writer
                    .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                    .expect("Failed to set color");
                writeln!(writer, "ok").expect("Failed to write to stderr");
            }
            TestStatus::Fail { message, error_diagnostic } => {
                writer
                    .set_color(ColorSpec::new().set_fg(Some(Color::Red)))
                    .expect("Failed to set color");
                writeln!(writer, "FAIL\n{message}\n").expect("Failed to write to stderr");
                if let Some(diag) = error_diagnostic {
                    noirc_errors::reporter::report_all(
                        file_manager.as_file_map(),
                        &[diag.clone()],
                        compile_options.deny_warnings,
                        compile_options.silence_warnings,
                    );
                }
            }
            TestStatus::CompileError(err) => {
                noirc_errors::reporter::report_all(
                    file_manager.as_file_map(),
                    &[err.clone()],
                    compile_options.deny_warnings,
                    compile_options.silence_warnings,
                );
            }
        }
        writer.reset().expect("Failed to reset writer");
    }

    write!(writer, "[{}] ", package.name).expect("Failed to write to stderr");

    let count_all = test_report.len();
    let count_failed = test_report.iter().filter(|(_, status)| status.failed()).count();
    let plural = if count_all == 1 { "" } else { "s" };
    if count_failed == 0 {
        writer.set_color(ColorSpec::new().set_fg(Some(Color::Green))).expect("Failed to set color");
        write!(writer, "{count_all} test{plural} passed").expect("Failed to write to stderr");
        writer.reset().expect("Failed to reset writer");
        writeln!(writer).expect("Failed to write to stderr");
    } else {
        let count_passed = count_all - count_failed;
        let plural_failed = if count_failed == 1 { "" } else { "s" };
        let plural_passed = if count_passed == 1 { "" } else { "s" };

        if count_passed != 0 {
            writer
                .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                .expect("Failed to set color");
            write!(writer, "{count_passed} test{plural_passed} passed, ",)
                .expect("Failed to write to stderr");
        }

        writer.set_color(ColorSpec::new().set_fg(Some(Color::Red))).expect("Failed to set color");
        writeln!(writer, "{count_failed} test{plural_failed} failed")
            .expect("Failed to write to stderr");
        writer.reset().expect("Failed to reset writer");
    }

    Ok(())
}
