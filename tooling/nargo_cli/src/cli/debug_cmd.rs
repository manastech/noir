use std::path::PathBuf;
use std::time::Duration;

use acvm::acir::circuit::ExpressionWidth;
use acvm::acir::native_types::{WitnessMap, WitnessStack};
use acvm::FieldElement;
use clap::Args;
use fm::FileManager;
use nargo::constants::PROVER_INPUT_FILE;
use nargo::errors::CompileError;
use nargo::ops::{
    compile_program, compile_program_with_debug_instrumenter, report_errors,
    test_status_program_compile_fail, test_status_program_compile_pass, TestStatus,
};
use nargo::package::{CrateName, Package};
use nargo::workspace::Workspace;
use nargo::{
    insert_all_files_for_workspace_into_file_manager, parse_all, prepare_package, NargoError,
};
use nargo_toml::PackageSelection;
use noirc_abi::input_parser::{Format, InputValue};
use noirc_abi::Abi;
use noirc_driver::{
    compile_no_check, file_manager_with_stdlib, link_to_debug_crate, CompileOptions,
    CompiledProgram,
};
use noirc_frontend::debug::DebugInstrumenter;
use noirc_frontend::graph::CrateId;
use noirc_frontend::hir::def_map::TestFunction;
use noirc_frontend::hir::{Context, FunctionNameMatch, ParsedFiles};

use super::check_cmd::check_crate_and_report_errors;
use super::compile_cmd::get_target_width;
use super::fs::{inputs::read_inputs_from_file, witness::save_witness_to_dir};
use super::test_cmd::formatters::Formatter;
use super::test_cmd::TestResult;
use super::{LockType, WorkspaceCommand};
use crate::cli::test_cmd::formatters::PrettyFormatter;
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
    pub(super) package: Option<CrateName>,

    #[clap(flatten)]
    compile_options: CompileOptions,

    /// Force ACIR output (disabling instrumentation)
    #[clap(long)]
    acir_mode: bool,

    /// Disable vars debug instrumentation (enabled by default)
    #[clap(long)]
    skip_instrumentation: Option<bool>,

    /// Raw string printing of source for testing
    #[clap(long, hide = true)]
    raw_source_printing: Option<bool>,

    /// Name (or substring) of the test function to debug
    #[clap(long)]
    test_name: Option<String>,

    /// JSON RPC url to solve oracle calls
    #[clap(long)]
    oracle_resolver: Option<String>,
}

struct RunParams {
    prover_name: String,
    witness_name: Option<String>,
    target_dir: PathBuf,
    // FIXME: perhaps this doesn't belong here
    // since it is for configuring the Bn254BlackBoxSolver
    // TODO: we should probably add the foreign call config in the same place
    pedantic_solving: bool,
    raw_source_printing: bool,
    oracle_resolver_url: Option<String>,
}

impl WorkspaceCommand for DebugCommand {
    fn package_selection(&self) -> PackageSelection {
        self.package
            .as_ref()
            .cloned()
            .map_or(PackageSelection::DefaultOrAll, PackageSelection::Selected)
    }

    fn lock_type(&self) -> LockType {
        // Always compiles fresh in-memory in debug mode, doesn't read or write the compilation artifacts.
        // Reads the Prover.toml file and writes the witness at the end, but shouldn't conflict with others.
        LockType::None
    }
}

pub(crate) fn run(args: DebugCommand, workspace: Workspace) -> Result<(), CliError> {
    let acir_mode = args.acir_mode;
    let skip_instrumentation = args.skip_instrumentation.unwrap_or(acir_mode);

    let run_params = RunParams {
        prover_name: args.prover_name,
        witness_name: args.witness_name,
        target_dir: workspace.target_directory_path(),
        pedantic_solving: args.compile_options.pedantic_solving,
        raw_source_printing: args.raw_source_printing.unwrap_or(false),
        oracle_resolver_url: args.oracle_resolver,
    };
    let workspace_clone = workspace.clone();

    let Some(package) = workspace_clone.into_iter().find(|p| p.is_binary() || p.is_contract())
    else {
        println!(
            "No matching binary or contract packages found in workspace. Only these packages can be debugged."
        );
        return Ok(());
    };

    let compile_options =
        compile_options_for_debugging(acir_mode, skip_instrumentation, args.compile_options);

    if let Some(test_name) = args.test_name {
        debug_test(test_name, package, workspace, compile_options, run_params)
    } else {
        debug_main(package, workspace, compile_options, run_params)
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
    expression_width: ExpressionWidth,
) -> Result<CompiledProgram, CompileError> {
    // TODO: extract fileManager creation + insert files into single function build_workspace_file_manager
    let mut workspace_file_manager = file_manager_with_stdlib(std::path::Path::new(""));
    insert_all_files_for_workspace_into_file_manager(workspace, &mut workspace_file_manager);

    let mut parsed_files = parse_all(&workspace_file_manager);

    let compilation_result = if compile_options.instrument_debug {
        let debug_state =
            instrument_package_files(&mut parsed_files, &workspace_file_manager, package);

        compile_program_with_debug_instrumenter(
            &workspace_file_manager,
            &parsed_files,
            workspace,
            package,
            &compile_options,
            None,
            debug_state,
        )
    } else {
        compile_program(
            &workspace_file_manager,
            &parsed_files,
            workspace,
            package,
            &compile_options,
            None,
        )
    };

    report_errors(
        compilation_result,
        &workspace_file_manager,
        compile_options.deny_warnings,
        compile_options.silence_warnings,
    )
    .map(|compiled_program| nargo::ops::transform_program(compiled_program, expression_width))
}

/// Add debugging instrumentation to all parsed files belonging to the package
/// being compiled
fn instrument_package_files(
    parsed_files: &mut ParsedFiles,
    file_manager: &FileManager,
    package: &Package,
) -> DebugInstrumenter {
    // Start off at the entry path and read all files in the parent directory.
    let entry_path_parent = package
        .entry_path
        .parent()
        .unwrap_or_else(|| panic!("The entry path is expected to be a single file within a directory and so should have a parent {:?}", package.entry_path));

    let mut debug_instrumenter = DebugInstrumenter::default();

    for (file_id, parsed_file) in parsed_files.iter_mut() {
        let file_path =
            file_manager.path(*file_id).expect("Parsed file ID not found in file manager");
        for ancestor in file_path.ancestors() {
            if ancestor == entry_path_parent {
                // file is in package
                debug_instrumenter.instrument_module(&mut parsed_file.0, *file_id);
            }
        }
    }

    debug_instrumenter
}

fn debug_main(
    package: &Package,
    workspace: Workspace,
    compile_options: CompileOptions,
    run_params: RunParams,
) -> Result<(), CliError> {
    let expression_width =
        get_target_width(package.expression_width, compile_options.expression_width);

    let compiled_program =
        compile_bin_package_for_debugging(&workspace, package, &compile_options, expression_width)?;

    run_async(package, compiled_program, &workspace, run_params).map(|_| ())
}

fn debug_test(
    test_name: String,
    package: &Package,
    workspace: Workspace,
    compile_options: CompileOptions,
    run_params: RunParams,
) -> Result<(), CliError> {
    let package_name = package.name.to_string();
    // let workspace_file_manager = build_workspace_file_manager(&workspace.root_dir, &workspace);
    // TODO: extract fileManager creation + insert files into single function build_workspace_file_manager
    let mut workspace_file_manager = file_manager_with_stdlib(std::path::Path::new(""));
    insert_all_files_for_workspace_into_file_manager(&workspace, &mut workspace_file_manager);

    let mut parsed_files = parse_all(&workspace_file_manager);
    let (mut context, crate_id) =
        prepare_package_for_debug(&workspace_file_manager, &mut parsed_files, package, &workspace);

    check_crate_and_report_errors(&mut context, crate_id, &compile_options)?;
    let (test_name, test_function) = get_test_function(crate_id, &context, &test_name)?;

    // TODO: see if we can replace with compile_bin_for_debugging
    let compiled_program =
        compile_no_check(&mut context, &compile_options, test_function.get_id(), None, false);

    let test_status = match compiled_program {
        Ok(compiled_program) => {
            // Run the backend to ensure the PWG evaluates functions like std::hash::pedersen,
            // otherwise constraints involving these expressions will not error.
            let expression_width =
                get_target_width(package.expression_width, compile_options.expression_width);
            let compiled_program =
                nargo::ops::transform_program(compiled_program, expression_width);

            let abi = compiled_program.abi.clone();
            let debug = compiled_program.debug.clone();

            // Debug test
            let debug_result = run_async(package, compiled_program, &workspace, run_params);

            match debug_result {
                Ok(result) => {
                    test_status_program_compile_pass(&test_function, &abi, &debug, &result)
                }
                // Debugger failed
                Err(error) => TestStatus::Fail {
                    message: format!("Debugger failed: {:?}", error),
                    error_diagnostic: None,
                },
            }
        }
        Err(err) => test_status_program_compile_fail(err, &test_function),
    };
    let test_result = TestResult::new(
        test_name,
        package_name,
        test_status,
        String::new(),
        Duration::from_secs(1), // FIXME: hardcoded value
    );

    let formatter: Box<dyn Formatter> = Box::new(PrettyFormatter);
    formatter
        .test_end_sync(&test_result, 1, 1, &workspace_file_manager, true, false, false)
        .expect("Could not display test result");

    Ok(())
}

// TODO: move to nargo::ops and reuse in test_cmd?
fn get_test_function(
    crate_id: CrateId,
    context: &Context,
    test_name: &str,
) -> Result<(String, TestFunction), CliError> {
    // TODO: review Contains signature and check if its ok to send test_name as single element
    let test_pattern = FunctionNameMatch::Contains(vec![test_name.into()]);

    let test_functions = context.get_all_test_functions_in_crate_matching(&crate_id, &test_pattern);

    let (test_name, test_function) = match test_functions {
        matchings if matchings.is_empty() => {
            return Err(CliError::Generic(format!(
                "`{}` does not match with any test function",
                test_name
            )));
        }
        matchings if matchings.len() == 1 => {
            let (name, test_func) = matchings.into_iter().next().unwrap();
            (name, test_func)
        }
        _ => {
            return Err(CliError::Generic(format!(
                "`{}` matches with more than one test function",
                test_name
            )));
        }
    };

    let test_function_has_arguments = !context
        .def_interner
        .function_meta(&test_function.get_id())
        .function_signature()
        .0
        .is_empty();

    if test_function_has_arguments {
        return Err(CliError::Generic(String::from("Cannot debug tests with arguments")));
    }
    Ok((test_name, test_function))
}

pub(crate) fn prepare_package_for_debug<'a>(
    file_manager: &'a FileManager,
    parsed_files: &'a mut ParsedFiles,
    package: &'a Package,
    workspace: &Workspace,
) -> (Context<'a, 'a>, CrateId) {
    let debug_instrumenter = instrument_package_files(parsed_files, file_manager, package);

    // -- This :down: is from nargo::ops(compile).compile_program_with_debug_instrumenter
    let (mut context, crate_id) = prepare_package(file_manager, parsed_files, package);
    link_to_debug_crate(&mut context, crate_id);
    context.debug_instrumenter = debug_instrumenter;
    context.package_build_path = workspace.package_build_path(package);
    (context, crate_id)
}

type DebugResult = Result<WitnessStack<FieldElement>, NargoError<FieldElement>>;
// FIXME: You shouldn't need this. CliError already has a variant which transparently can carry a NargoError.
type ExecutionResult =
    Result<(Option<InputValue>, WitnessStack<FieldElement>), NargoError<FieldElement>>;
fn run_async(
    package: &Package,
    program: CompiledProgram,
    workspace: &Workspace,
    run_params: RunParams,
) -> Result<DebugResult, CliError> {
    use tokio::runtime::Builder;
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();

    runtime.block_on(async {
        println!("[{}] Starting debugger", package.name);
        let debug_result = debug_program_and_decode(program, package, workspace, &run_params)?;

        match debug_result {
            Ok((return_value, witness_stack)) => {
                let witness_stack_result = witness_stack.clone();
                println!("[{}] Circuit witness successfully solved", package.name);

                if let Some(return_value) = return_value {
                    println!("[{}] Circuit output: {return_value:?}", package.name);
                }

                if let Some(witness_name) = run_params.witness_name {
                    let witness_path = match save_witness_to_dir(
                        witness_stack,
                        &witness_name,
                        run_params.target_dir,
                    ) {
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

// FIXME: We have nested results to differentiate between the execution result (the inner one - Nargo)
// and setting up the debugger errors (outer one - CliErrors)
fn debug_program_and_decode(
    program: CompiledProgram,
    package: &Package,
    workspace: &Workspace,
    run_params: &RunParams,
) -> Result<ExecutionResult, CliError> {
    let program_abi = program.abi.clone();
    let initial_witness = parse_initial_witness(package, &run_params.prover_name, &program.abi)?;
    let debug_result = debug_program(
        program,
        initial_witness,
        run_params.pedantic_solving,
        run_params.raw_source_printing,
        run_params.oracle_resolver_url.clone(),
        Some(workspace.root_dir.clone()),
        package.name.to_string(),
    );
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
    pedantic_solving: bool,
    raw_source_printing: bool,
    foreign_call_resolver_url: Option<String>,
    root_path: Option<PathBuf>,
    package_name: String,
) -> Result<Option<WitnessStack<FieldElement>>, NargoError<FieldElement>> {
    noir_debugger::run_repl_session(
        compiled_program,
        initial_witness,
        raw_source_printing,
        foreign_call_resolver_url,
        root_path,
        package_name,
        pedantic_solving,
    )
}
