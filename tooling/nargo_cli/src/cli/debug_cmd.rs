use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use acvm::FieldElement;
use acvm::acir::circuit::ExpressionWidth;
use acvm::acir::native_types::{WitnessMap, WitnessStack};
use clap::Args;
use fm::FileManager;
use nargo::constants::PROVER_INPUT_FILE;
use nargo::errors::CompileError;
use nargo::ops::{
    TestStatus, compile_program, compile_program_with_debug_instrumenter, report_errors,
    test_status_program_compile_fail, test_status_program_compile_pass,
};
use nargo::package::{CrateName, Package};
use nargo::workspace::Workspace;
use nargo::{insert_all_files_for_workspace_into_file_manager, parse_all, prepare_package};
use nargo_toml::PackageSelection;
use noir_artifact_cli::fs::inputs::read_inputs_from_file;
use noir_artifact_cli::fs::witness::save_witness_to_dir;
use noir_debugger::DebugExecutionResult;
use noirc_abi::Abi;
use noirc_driver::{
    CompileOptions, CompiledProgram, compile_no_check, file_manager_with_stdlib,
    link_to_debug_crate,
};
use noirc_frontend::debug::DebugInstrumenter;
use noirc_frontend::graph::CrateId;
use noirc_frontend::hir::def_map::TestFunction;
use noirc_frontend::hir::{Context, FunctionNameMatch, ParsedFiles};

use super::check_cmd::check_crate_and_report_errors;
use super::compile_cmd::get_target_width;
use super::test_cmd::TestResult;
use super::test_cmd::formatters::Formatter;
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

struct RunParams<'a> {
    prover_name: String,
    witness_name: Option<String>,
    target_dir: &'a Path,
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
        target_dir: &workspace.target_directory_path(),
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

fn print_test_result(test_result: TestResult, file_manager: &FileManager) {
    let formatter: Box<dyn Formatter> = Box::new(PrettyFormatter);
    formatter
        .test_end_sync(&test_result, 1, 1, file_manager, true, false, false)
        .expect("Could not display test result");
}

fn debug_test_fn(
    test: &TestDefinition,
    context: &mut Context,
    workspace: &Workspace,
    package: &Package,
    compile_options: CompileOptions,
    run_params: RunParams,
) -> TestResult {
    let compiled_program =
        compile_test_fn_for_debugging(test, context, package, compile_options, None);

    let test_status = match compiled_program {
        Ok(compiled_program) => {
            let abi = compiled_program.abi.clone();
            let debug = compiled_program.debug.clone();

            // Run debugger
            let debug_result = run_async(package, compiled_program, workspace, run_params);

            match debug_result {
                Ok(DebugExecutionResult::Solved(result)) => {
                    test_status_program_compile_pass(&test.function, &abi, &debug, &Ok(result))
                }
                Ok(DebugExecutionResult::Error(error)) => {
                    test_status_program_compile_pass(&test.function, &abi, &debug, &Err(error))
                }
                Ok(DebugExecutionResult::Incomplete) => TestStatus::Fail {
                    message: "Debugger halted. Incomplete execution".to_string(),
                    error_diagnostic: None,
                },
                Err(error) => TestStatus::Fail {
                    message: format!("Debugger failed: {error}"),
                    error_diagnostic: None,
                },
            }
        }
        Err(err) => test_status_program_compile_fail(err, &test.function),
    };

    TestResult::new(
        test.name.clone(),
        package.name.to_string(),
        test_status,
        String::new(),
        Duration::from_secs(1), // FIXME: hardcoded value
    )
}

pub(super) fn compile_test_fn_for_debugging(
    test_def: &TestDefinition,
    context: &mut Context,
    package: &Package,
    compile_options: CompileOptions,
    expression_with: Option<ExpressionWidth>,
) -> Result<CompiledProgram, noirc_driver::CompileError> {
    let compiled_program =
        compile_no_check(context, &compile_options, test_def.function.get_id(), None, false)?;
    let expression_width = expression_with
        .unwrap_or(get_target_width(package.expression_width, compile_options.expression_width));
    let compiled_program = nargo::ops::transform_program(compiled_program, expression_width);
    Ok(compiled_program)
}

pub(crate) fn compile_bin_package_for_debugging(
    workspace: &Workspace,
    package: &Package,
    compile_options: &CompileOptions,
    expression_width: ExpressionWidth,
) -> Result<CompiledProgram, CompileError> {
    let (workspace_file_manager, mut parsed_files) = load_workspace_files(workspace);

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

    run_async(package, compiled_program, &workspace, run_params)?;

    Ok(())
}

fn debug_test(
    test_name: String,
    package: &Package,
    workspace: Workspace,
    compile_options: CompileOptions,
    run_params: RunParams,
) -> Result<(), CliError> {
    let (file_manager, mut parsed_files) = load_workspace_files(&workspace);

    let (mut context, crate_id) =
        prepare_package_for_debug(&file_manager, &mut parsed_files, package, &workspace);

    check_crate_and_report_errors(&mut context, crate_id, &compile_options)?;

    let test = get_test_function(crate_id, &context, &test_name)?;

    let test_result =
        debug_test_fn(&test, &mut context, &workspace, package, compile_options, run_params);
    print_test_result(test_result, &file_manager);

    Ok(())
}

pub(super) struct TestDefinition {
    pub name: String,
    pub function: TestFunction,
}

// TODO: move to nargo::ops and reuse in test_cmd?
pub(super) fn get_test_function(
    crate_id: CrateId,
    context: &Context,
    test_name: &str,
) -> Result<TestDefinition, CliError> {
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
    Ok(TestDefinition { name: test_name, function: test_function })
}

pub(super) fn load_workspace_files(workspace: &Workspace) -> (FileManager, ParsedFiles) {
    let mut file_manager = file_manager_with_stdlib(Path::new(""));
    insert_all_files_for_workspace_into_file_manager(workspace, &mut file_manager);

    let parsed_files = parse_all(&file_manager);
    (file_manager, parsed_files)
}
pub(super) fn prepare_package_for_debug<'a>(
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

fn run_async(
    package: &Package,
    program: CompiledProgram,
    workspace: &Workspace,
    run_params: RunParams,
) -> Result<DebugExecutionResult, CliError> {
    use tokio::runtime::Builder;
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let abi = &program.abi.clone();

    runtime.block_on(async {
        println!("[{}] Starting debugger", package.name);
        let initial_witness = parse_initial_witness(package, &run_params.prover_name, abi)?;

        let result = debug_program(
            program,
            initial_witness,
            run_params.pedantic_solving,
            run_params.raw_source_printing,
            run_params.oracle_resolver_url,
            Some(workspace.root_dir.clone()),
            package.name.to_string(),
        );

        if let DebugExecutionResult::Solved(ref witness_stack) = result {
            println!("[{}] Circuit witness successfully solved", package.name);
            decode_and_save_program_witness(
                &package.name,
                witness_stack,
                abi,
                run_params.witness_name,
                &run_params.target_dir,
            )?;
        }

        Ok(result)
    })
}

fn decode_and_save_program_witness(
    package_name: &CrateName,
    witness_stack: &WitnessStack<FieldElement>,
    abi: &Abi,
    target_witness_name: Option<String>,
    target_dir: &Path,
) -> Result<(), CliError> {
    let main_witness =
        &witness_stack.peek().expect("Should have at least one witness on the stack").witness;

    if let (_, Some(return_value)) = abi.decode(main_witness)? {
        println!("[{}] Circuit output: {return_value:?}", package_name);
    }

    if let Some(witness_name) = target_witness_name {
        let witness_path = save_witness_to_dir(witness_stack, &witness_name, target_dir)?;
        println!("[{}] Witness saved to {}", package_name, witness_path.display());
    }
    Ok(())
}

fn parse_initial_witness(
    package: &Package,
    prover_name: &str,
    abi: &Abi,
) -> Result<WitnessMap<FieldElement>, CliError> {
    // Parse the initial witness values from Prover.toml
    let (inputs_map, _) =
        read_inputs_from_file(&package.root_dir.join(prover_name).with_extension("toml"), abi)?;
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
) -> DebugExecutionResult {
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
