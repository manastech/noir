use acvm::FieldElement;
use acvm::acir::circuit::ExpressionWidth;
use acvm::acir::native_types::WitnessMap;
use clap::Args;
use nargo::constants::PROVER_INPUT_FILE;
use nargo::package::Package;
use nargo::workspace::Workspace;
use nargo_toml::{PackageSelection, get_package_manifest, resolve_workspace_from_toml};
use noir_artifact_cli::fs::inputs::read_inputs_from_file;
use noirc_driver::{CompileOptions, CompiledProgram, NOIR_ARTIFACT_VERSION_STRING};
use noirc_frontend::graph::CrateName;

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use dap::requests::Command;
use dap::responses::ResponseBody;
use dap::server::Server;
use dap::types::Capabilities;
use serde_json::Value;

use super::check_cmd::check_crate_and_report_errors;
use super::debug_cmd::{compile_bin_package_for_debugging, compile_options_for_debugging, compile_test_fn_for_debugging, get_test_function, load_workspace_files, prepare_package_for_debug};
use crate::errors::CliError;

use noir_debugger::errors::{DapError, LoadError};

#[derive(Debug, Clone, Args)]
pub(crate) struct DapCommand {
    /// Override the expression width requested by the backend.
    #[arg(long, value_parser = parse_expression_width, default_value = "4")]
    expression_width: ExpressionWidth,

    #[clap(long)]
    preflight_check: bool,

    #[clap(long)]
    preflight_project_folder: Option<String>,

    #[clap(long)]
    preflight_package: Option<String>,

    #[clap(long)]
    preflight_prover_name: Option<String>,

    #[clap(long)]
    preflight_generate_acir: bool,

    #[clap(long)]
    preflight_skip_instrumentation: bool,

    #[clap(long)]
    preflight_test_name: Option<String>,

    /// Use pedantic ACVM solving, i.e. double-check some black-box function
    /// assumptions when solving.
    /// This is disabled by default.
    #[arg(long, default_value = "false")]
    pedantic_solving: bool,
}

fn parse_expression_width(input: &str) -> Result<ExpressionWidth, std::io::Error> {
    use std::io::{Error, ErrorKind};

    let width = input
        .parse::<usize>()
        .map_err(|err| Error::new(ErrorKind::InvalidInput, err.to_string()))?;

    match width {
        0 => Ok(ExpressionWidth::Unbounded),
        _ => Ok(ExpressionWidth::Bounded { width }),
    }
}

fn find_workspace(project_folder: &str, package: Option<&str>) -> Option<Workspace> {
    let Ok(toml_path) = get_package_manifest(Path::new(project_folder)) else {
        eprintln!("ERROR: Failed to get package manifest");
        return None;
    };
    let package = package.and_then(|p| serde_json::from_str::<CrateName>(p).ok());
    let selection = package.map_or(PackageSelection::DefaultOrAll, PackageSelection::Selected);
    match resolve_workspace_from_toml(
        &toml_path,
        selection,
        Some(NOIR_ARTIFACT_VERSION_STRING.to_string()),
    ) {
        Ok(workspace) => Some(workspace),
        Err(err) => {
            eprintln!("ERROR: Failed to resolve workspace: {err}");
            None
        }
    }
}

fn workspace_not_found_error_msg(project_folder: &str, package: Option<&str>) -> String {
    match package {
        Some(pkg) => format!(
            r#"Noir Debugger could not load program from {}, package {}"#,
            project_folder, pkg
        ),
        None => format!(r#"Noir Debugger could not load program from {}"#, project_folder),
    }
}

fn compile_main(
    workspace: &Workspace,
    package: &Package,
    expression_width: ExpressionWidth,
    compile_options: &CompileOptions,
) -> Result<CompiledProgram, LoadError> {
    compile_bin_package_for_debugging(&workspace, package, &compile_options, expression_width)
        .map_err(|_| LoadError::Generic("Failed to compile project".into()))
}

fn compile_test(
    workspace: &Workspace,
    package: &Package,
    expression_width: ExpressionWidth,
    compile_options: CompileOptions,
    test_name: String,
) -> Result<CompiledProgram, LoadError> {
    let (file_manager, mut parsed_files) = load_workspace_files(&workspace);

    let (mut context, crate_id) =
        prepare_package_for_debug(&file_manager, &mut parsed_files, package, &workspace);

    check_crate_and_report_errors(&mut context, crate_id, &compile_options).map_err(|_| LoadError::Generic("Failed to compile project".into()))?;

    let test = get_test_function(crate_id, &context, &test_name).map_err(|_| LoadError::Generic("Failed to compile project".into()))?;

    compile_test_fn_for_debugging(&test, &mut context, package, compile_options, Some(expression_width)).map_err(|_| LoadError::Generic("Failed to compile project".into()))

}

fn load_and_compile_project(
    project_folder: &str,
    package: Option<&str>,
    prover_name: &str,
    expression_width: ExpressionWidth,
    acir_mode: bool,
    skip_instrumentation: bool,
    test_name: Option<String>,
) -> Result<(CompiledProgram, WitnessMap<FieldElement>, PathBuf, String), LoadError> {
    let workspace = find_workspace(project_folder, package)
        .ok_or(LoadError::Generic(workspace_not_found_error_msg(project_folder, package)))?;
    let package = workspace
        .into_iter()
        .find(|p| p.is_binary() || p.is_contract())
        .ok_or(LoadError::Generic("No matching binary or contract packages found in workspace. Only these packages can be debugged.".into()))?;

    let compile_options =
        compile_options_for_debugging(acir_mode, skip_instrumentation, CompileOptions::default());

    let compiled_program = match test_name {
        None => compile_main(&workspace, &package, expression_width, &compile_options),
        Some(test_name) => compile_test(&workspace, &package, expression_width, compile_options, test_name),
    }?;


    let (inputs_map, _) = read_inputs_from_file(
        &package.root_dir.join(prover_name).with_extension("toml"),
        &compiled_program.abi,
    )
    .map_err(|e| {
        LoadError::Generic(format!("Failed to read program inputs from {prover_name}: {e}"))
    })?;
    let initial_witness = compiled_program
        .abi
        .encode(&inputs_map, None)
        .map_err(|_| LoadError::Generic("Failed to encode inputs".into()))?;

    Ok((compiled_program, initial_witness, workspace.root_dir.clone(), package.name.to_string()))
}

fn loop_uninitialized_dap<R: Read, W: Write>(
    mut server: Server<R, W>,
    expression_width: ExpressionWidth,
    pedantic_solving: bool,
) -> Result<(), DapError> {
    while let Some(req) = server.poll_request()? {
        match req.command {
            Command::Initialize(_) => {
                let rsp = req.success(ResponseBody::Initialize(Capabilities {
                    supports_disassemble_request: Some(true),
                    supports_instruction_breakpoints: Some(true),
                    supports_stepping_granularity: Some(true),
                    ..Default::default()
                }));
                server.respond(rsp)?;
            }

            Command::Launch(ref arguments) => {
                let Some(Value::Object(ref additional_data)) = arguments.additional_data else {
                    server.respond(req.error("Missing launch arguments"))?;
                    continue;
                };
                let Some(Value::String(project_folder)) = additional_data.get("projectFolder")
                else {
                    server.respond(req.error("Missing project folder argument"))?;
                    continue;
                };

                let project_folder = project_folder.as_str();
                let package = additional_data.get("package").and_then(|v| v.as_str());
                let prover_name = additional_data
                    .get("proverName")
                    .and_then(|v| v.as_str())
                    .unwrap_or(PROVER_INPUT_FILE);

                let generate_acir =
                    additional_data.get("generateAcir").and_then(|v| v.as_bool()).unwrap_or(false);
                let skip_instrumentation = additional_data
                    .get("skipInstrumentation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(generate_acir);
                let test_name =
                    additional_data.get("testName").and_then(|v| v.as_str()).map(String::from);
                let oracle_resolver_url = additional_data
                    .get("oracleResolver")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                eprintln!("Project folder: {}", project_folder);
                eprintln!("Package: {}", package.unwrap_or("(default)"));
                eprintln!("Prover name: {}", prover_name);

                match load_and_compile_project(
                    project_folder,
                    package,
                    prover_name,
                    expression_width,
                    generate_acir,
                    skip_instrumentation,
                    test_name,
                ) {
                    Ok((compiled_program, initial_witness, root_path, package_name)) => {
                        server.respond(req.ack()?)?;

                        noir_debugger::run_dap_loop(
                            server,
                            compiled_program,
                            initial_witness,
                            Some(root_path),
                            package_name,
                            pedantic_solving,
                            oracle_resolver_url,
                        )?;
                        break;
                    }
                    Err(LoadError::Generic(message)) => {
                        server.respond(req.error(message.as_str()))?;
                    }
                }
            }

            Command::Disconnect(_) => {
                server.respond(req.ack()?)?;
                break;
            }

            _ => {
                let command = req.command;
                eprintln!("ERROR: unhandled command: {command:?}");
            }
        }
    }
    Ok(())
}

fn run_preflight_check(
    expression_width: ExpressionWidth,
    args: DapCommand,
) -> Result<(), DapError> {
    let project_folder = if let Some(project_folder) = args.preflight_project_folder {
        project_folder
    } else {
        return Err(DapError::PreFlightGenericError("Noir Debugger could not initialize because the IDE (for example, VS Code) did not specify a project folder to debug.".into()));
    };

    let package = args.preflight_package.as_deref();
    let test_name = args.preflight_test_name;
    let prover_name = args.preflight_prover_name.as_deref().unwrap_or(PROVER_INPUT_FILE);

    let _ = load_and_compile_project(
        project_folder.as_str(),
        package,
        prover_name,
        expression_width,
        args.preflight_generate_acir,
        args.preflight_skip_instrumentation,
        test_name,
    )?;

    Ok(())
}

pub(crate) fn run(args: DapCommand) -> Result<(), CliError> {
    // When the --preflight-check flag is present, we run Noir's DAP server in "pre-flight mode", which test runs
    // the DAP initialization code without actually starting the DAP server.
    //
    // This lets the client IDE present any initialization issues (compiler version mismatches, missing prover files, etc)
    // in its own interface.
    //
    // This was necessary due to the VS Code project being reluctant to let extension authors capture
    // stderr output generated by a DAP server wrapped in DebugAdapterExecutable.
    //
    // Exposing this preflight mode lets us gracefully handle errors that happen *before*
    // the DAP loop is established, which otherwise are considered "out of band" by the maintainers of the DAP spec.
    // More details here: https://github.com/microsoft/vscode/issues/108138
    if args.preflight_check {
        return run_preflight_check(args.expression_width, args).map_err(CliError::DapError);
    }

    let output = BufWriter::new(std::io::stdout());
    let input = BufReader::new(std::io::stdin());
    let server = Server::new(input, output);

    loop_uninitialized_dap(server, args.expression_width, args.pedantic_solving)
        .map_err(CliError::DapError)
}
