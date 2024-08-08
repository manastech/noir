use std::path::PathBuf;

use acvm::acir::native_types::{WitnessMap, WitnessStack};
use acvm::FieldElement;
use bn254_blackbox_solver::Bn254BlackBoxSolver;
use clap::Args;

use fm::FileManager;
use nargo::constants::PROVER_INPUT_FILE;
use nargo::errors::CompileError;
use nargo::ops::{
    compile_program, compile_program_with_debug_instrumenter, report_errors,
    test_status_program_compile_fail, test_status_program_compile_pass, TestStatus,
};
use nargo::package::Package;
use nargo::workspace::Workspace;
use nargo::{prepare_package, NargoError};
use nargo_toml::{get_package_manifest, resolve_workspace_from_toml, PackageSelection};

use noirc_abi::input_parser::{Format, InputValue};
use noirc_abi::Abi;
use noirc_driver::{
    check_crate, compile_no_check, link_to_debug_crate, CompileOptions, CompiledProgram,
    NOIR_ARTIFACT_VERSION_STRING,
};
use noirc_frontend::graph::{CrateId, CrateName};
use noirc_frontend::hir::{Context, FunctionNameMatch, ParsedFiles};

use super::compile_cmd::get_target_width;
use super::execution_helpers::{file_manager_and_files_from, instrument_package_files};
use super::fs::{inputs::read_inputs_from_file, witness::save_witness_to_dir};
use super::test_cmd::{display_test_report, get_tests_in_package};
use super::NargoConfig;
use crate::errors::CliError;

/// Executes a circuit in debug mode
#[derive(Debug, Clone, Args)]
pub(crate) struct DebugCommand {
    /// Write the execution witness to named file
    witness_name: Option<String>,

    /// The name of the toml file which contains the inputs for the prover
    #[clap(long, short, default_value = PROVER_INPUT_FILE)]
    prover_name: String,

    /// The name of the package to execute
    #[clap(long)]
    package: Option<CrateName>,

    #[clap(flatten)]
    compile_options: CompileOptions,

    /// Force ACIR output (disabling instrumentation)
    #[clap(long)]
    acir_mode: bool,

    /// Only run tests that match exactly
    #[clap(long)]
    test_name: Option<String>,

    /// Disable vars debug instrumentation (enabled by default)
    #[clap(long)]
    skip_instrumentation: Option<bool>,
}

pub(crate) fn run(args: DebugCommand, config: NargoConfig) -> Result<(), CliError> {
    let acir_mode = args.acir_mode;
    let skip_instrumentation = args.skip_instrumentation.unwrap_or(acir_mode);

    let toml_path = get_package_manifest(&config.program_dir)?;
    let selection = args.package.map_or(PackageSelection::DefaultOrAll, PackageSelection::Selected);
    let workspace = resolve_workspace_from_toml(
        &toml_path,
        selection,
        Some(NOIR_ARTIFACT_VERSION_STRING.to_string()),
    )?;
    let target_dir = &workspace.target_directory_path();
    let workspace_clone = workspace.clone();
    let Some(package) = workspace_clone.into_iter().find(|p| p.is_binary()) else {
        println!(
            "No matching binary packages found in workspace. Only binary packages can be debugged."
        );
        return Ok(());
    };
    let compile_options =
        compile_options_for_debugging(acir_mode, skip_instrumentation, args.compile_options);

    match args.test_name {
        Some(test_name) => run_async_for_test(
            test_name,
            package,
            workspace,
            &args.prover_name,
            &args.witness_name,
            target_dir,
            &compile_options,
        ),
        None => {
            let compiled_program =
                compile_bin_package_for_debugging(&workspace, package, &compile_options)?;

            let target_width =
                get_target_width(package.expression_width, compile_options.expression_width);

            let compiled_program = nargo::ops::transform_program(compiled_program, target_width);
            run_async(package, compiled_program, &args.prover_name, &args.witness_name, target_dir)
                .map(|_| ())
        }
    }
}

pub(crate) fn compile_options_for_debugging(
    acir_mode: bool,
    skip_instrumentation: bool,
    compile_options: CompileOptions,
) -> CompileOptions {
    CompileOptions {
        instrument_debug: !skip_instrumentation,
        force_brillig: !acir_mode,
        ..compile_options
    }
}

pub(crate) fn compile_bin_package_for_debugging(
    workspace: &Workspace,
    package: &Package,
    compile_options: &CompileOptions,
) -> Result<CompiledProgram, CompileError> {
    let (workspace_file_manager, mut parsed_files) =
        file_manager_and_files_from(std::path::Path::new(""), workspace);

    let compilation_result = if compile_options.instrument_debug {
        let debug_state =
            instrument_package_files(&mut parsed_files, &workspace_file_manager, package);

        compile_program_with_debug_instrumenter(
            &workspace_file_manager,
            &parsed_files,
            workspace,
            package,
            compile_options,
            None,
            debug_state,
        )
    } else {
        compile_program(
            &workspace_file_manager,
            &parsed_files,
            workspace,
            package,
            compile_options,
            None,
        )
    };

    report_errors(
        compilation_result,
        &workspace_file_manager,
        compile_options.deny_warnings,
        compile_options.silence_warnings,
    )
}

// TODO: rename
fn run_async_for_test(
    test_name: String,
    package: &Package,
    workspace: Workspace,
    prover_name: &str,
    witness_name: &Option<String>,
    target_dir: &PathBuf,
    compile_options: &CompileOptions,
) -> Result<(), CliError> {
    let test_pattern = FunctionNameMatch::Exact(&test_name);
    let (workspace_file_manager, mut parsed_files) =
        file_manager_and_files_from(&workspace.root_dir, &workspace);
    let test_functions = get_tests_in_package(
        &workspace_file_manager,
        &parsed_files,
        package,
        test_pattern,
        compile_options,
    )?;
    let more_than_one_match = test_functions.len() > 1;
    if more_than_one_match {
        println!(
            "[{}] Ignoring --debug flag since debugging multiple test is disabled",
            package.name
        );
        return Err(CliError::Generic(String::from(
            "test_name argument matches more than one test function",
        )));
    };
    let (mut context, crate_id) =
        prepare_package_for_debug(&workspace_file_manager, &mut parsed_files, package);
    check_crate(&mut context, crate_id, compile_options)
        .expect("Any errors should have occurred when collecting test functions");

    let test_functions = context.get_all_test_functions_in_crate_matching(&crate_id, test_pattern);
    let (_, test_function) = test_functions.first().expect("Test function should exist");

    let test_function_has_arguments = !context
        .def_interner
        .function_meta(&test_function.get_id())
        .function_signature()
        .0
        .is_empty();

    if test_function_has_arguments {
        println!(
            "[{}] Ignoring --debug flag since debugging test with arguments is disabled",
            package.name
        );
        return Err(CliError::Generic(String::from("Cannot debug tests with arguments")));
    }

    let compiled_program =
        compile_no_check(&mut context, compile_options, test_function.get_id(), None, false);

    let test_status = match compiled_program {
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
            let debug_result =
                run_async(package, compiled_program, prover_name, witness_name, target_dir);

            match debug_result {
                Ok(result) => test_status_program_compile_pass(test_function, abi, debug, result),
                Err(error) => TestStatus::Fail {
                    message: format!("Debugger failed: {}", error),
                    error_diagnostic: None,
                },
            }
        }
        Err(err) => test_status_program_compile_fail(err, test_function),
    };

    display_test_report(
        &workspace_file_manager,
        package,
        compile_options,
        &[(test_name, test_status)],
    )
}

fn run_async(
    package: &Package,
    program: CompiledProgram,
    prover_name: &str,
    witness_name: &Option<String>,
    target_dir: &PathBuf,
) -> Result<DebugResult, CliError> {
    use tokio::runtime::Builder;
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();

    runtime.block_on(async {
        println!("[{}] Starting debugger", package.name);
        let debug_result = debug_program_and_decode(program, package, prover_name)?;

        match debug_result {
            Ok((return_value, witness_stack)) => {
                let witness_stack_result = witness_stack.clone();
                println!("[{}] Circuit witness successfully solved", package.name);

                if let Some(return_value) = return_value {
                    println!("[{}] Circuit output: {return_value:?}", package.name);
                }

                if let Some(witness_name) = witness_name {
                    let witness_path =
                        match save_witness_to_dir(witness_stack, witness_name, target_dir) {
                            Ok(path) => path,
                            Err(err) => return Err(CliError::from(err)),
                        };

                    println!("[{}] Witness saved to {}", package.name, witness_path.display());
                }
                Ok(Ok(witness_stack_result))
            }
            Err(error) => Ok(Err(error)),
        }
    })
}

type DebugResult = Result<WitnessStack<FieldElement>, NargoError<FieldElement>>;
type ExecutionResult =
    Result<(Option<InputValue>, WitnessStack<FieldElement>), NargoError<FieldElement>>;

// FIXME: We have nested results to differentiate between the execution result (the inner one - Nargo)
// and setting up the debugger errors (outer one - CliErrors)
fn debug_program_and_decode(
    program: CompiledProgram,
    package: &Package,
    prover_name: &str,
) -> Result<ExecutionResult, CliError> {
    let program_abi = program.abi.clone();

    let initial_witness = parse_initial_witness(package, prover_name, &program.abi)?;
    let debug_result = debug_program(program, initial_witness);

    match debug_result {
        Ok(witness_stack) => match witness_stack {
            Some(witness_stack) => {
                let main_witness = &witness_stack
                    .peek()
                    .expect("Should have at least one witness on the stack")
                    .witness;
                let (_, return_value) = program_abi.decode(main_witness)?;
                Ok(Ok((return_value, witness_stack)))
            }
            None => Err(CliError::ExecutionHalted),
        },
        Err(error) => Ok(Err(error)),
    }
}

fn parse_initial_witness(
    package: &Package,
    prover_name: &str,
    abi: &Abi,
) -> Result<WitnessMap<FieldElement>, CliError> {
    // Parse the initial witness values from Prover.toml
    let (inputs_map, _) = read_inputs_from_file(&package.root_dir, prover_name, Format::Toml, abi)?;
    let initial_witness = abi.encode(&inputs_map, None)?;
    Ok(initial_witness)
}
pub(crate) fn debug_program(
    compiled_program: CompiledProgram,
    initial_witness: WitnessMap<FieldElement>,
) -> Result<Option<WitnessStack<FieldElement>>, NargoError<FieldElement>> {
    noir_debugger::run_repl_session(&Bn254BlackBoxSolver, compiled_program, initial_witness)
}

pub(crate) fn prepare_package_for_debug<'a>(
    file_manager: &'a FileManager,
    parsed_files: &'a mut ParsedFiles,
    package: &'a Package,
) -> (Context<'a, 'a>, CrateId) {
    let debug_instrumenter = instrument_package_files(parsed_files, file_manager, package);

    // -- This :down: is from nargo::ops(compile).compile_program_with_debug_instrumenter
    let (mut context, crate_id) = prepare_package(file_manager, parsed_files, package);
    link_to_debug_crate(&mut context, crate_id);
    context.debug_instrumenter = debug_instrumenter;
    (context, crate_id)
}
