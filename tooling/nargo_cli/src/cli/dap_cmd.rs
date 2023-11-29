use acvm::acir::native_types::WitnessMap;
use backend_interface::Backend;
use clap::Args;
use nargo::constants::PROVER_INPUT_FILE;
use nargo::workspace::Workspace;
use nargo_toml::{get_package_manifest, resolve_workspace_from_toml, PackageSelection};
use noirc_abi::input_parser::Format;
use noirc_driver::{CompileOptions, CompiledProgram, NOIR_ARTIFACT_VERSION_STRING};
use noirc_frontend::graph::CrateName;

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use dap::errors::ServerError;
use dap::requests::Command;
use dap::responses::ResponseBody;
use dap::server::Server;
use dap::types::Capabilities;
use serde_json::Value;

use super::compile_cmd::compile_bin_package;
use super::fs::inputs::read_inputs_from_file;
use crate::errors::CliError;

use super::NargoConfig;

#[derive(Debug, Clone, Args)]
pub(crate) struct DapCommand;

struct LoadError(&'static str);

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

fn load_and_compile_project(
    backend: &Backend,
    project_folder: &str,
    package: Option<&str>,
    prover_name: &str,
) -> Result<(CompiledProgram, WitnessMap), LoadError> {
    let Some(workspace) = find_workspace(project_folder, package) else {
        return Err(LoadError("Cannot open workspace"));
    };
    let Ok((np_language, opcode_support)) = backend.get_backend_info() else {
        return Err(LoadError("Failed to get backend info"));
    };
    let Some(package) = workspace.into_iter().find(|p| p.is_binary()) else {
        return Err(LoadError("No matching binary packages found in workspace"));
    };

    let Ok(compiled_program) = compile_bin_package(
        &workspace,
        package,
        &CompileOptions::default(),
        np_language,
        &opcode_support,
    ) else {
        return Err(LoadError("Failed to compile project"));
    };
    let Ok((inputs_map, _)) = read_inputs_from_file(
        &package.root_dir,
        prover_name,
        Format::Toml,
        &compiled_program.abi,
    ) else {
        return Err(LoadError("Failed to read program inputs"));
    };
    let Ok(initial_witness) = compiled_program.abi.encode(&inputs_map, None) else {
        return Err(LoadError("Failed to encode inputs"));
    };

    Ok((compiled_program, initial_witness))
}

fn loop_uninitialized_dap<R: Read, W: Write>(
    mut server: Server<R, W>,
    backend: &Backend,
) -> Result<(), ServerError> {
    loop {
        let req = match server.poll_request()? {
            Some(req) => req,
            None => break,
        };

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
                let Some(Value::String(ref project_folder)) = additional_data.get("projectFolder") else {
                    server.respond(req.error("Missing project folder argument"))?;
                    continue;
                };

                let project_folder = project_folder.as_str();
                let package = additional_data.get("package").and_then(|v| v.as_str());
                let prover_name = additional_data
                    .get("proverName")
                    .and_then(|v| v.as_str())
                    .unwrap_or(PROVER_INPUT_FILE);

                eprintln!("Project folder: {}", project_folder);
                eprintln!("Package: {}", package.unwrap_or("(default)"));
                eprintln!("Prover name: {}", prover_name);

                match load_and_compile_project(backend, project_folder, package, prover_name) {
                    Ok((compiled_program, initial_witness)) => {
                        server.respond(req.ack()?)?;

                        #[allow(deprecated)]
                        let blackbox_solver =
                            barretenberg_blackbox_solver::BarretenbergSolver::new();

                        noir_debugger::run_dap_loop(
                            server,
                            &blackbox_solver,
                            compiled_program,
                            initial_witness,
                        )?;
                        break;
                    }
                    Err(LoadError(message)) => {
                        server.respond(req.error(message))?;
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

pub(crate) fn run(
    backend: &Backend,
    _args: DapCommand,
    _config: NargoConfig,
) -> Result<(), CliError> {
    let output = BufWriter::new(std::io::stdout());
    let input = BufReader::new(std::io::stdin());
    let server = Server::new(input, output);

    loop_uninitialized_dap(server, backend).map_err(CliError::DapError)
}
