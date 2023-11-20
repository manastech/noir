#![warn(unused_crate_dependencies, unused_extern_crates)]
#![warn(unreachable_pub)]
#![warn(clippy::semicolon_if_nothing_returned)]

mod errors;
mod js_witness_map;

use acvm::{
    acir::{
        circuit::Circuit,
        native_types::WitnessMap,
    },
    pwg::{ACVMStatus, ErrorLocation, OpcodeResolutionError, ACVM},
};

// See Cargo.toml for explanation.
use getrandom as _;

use noir_debugger::debug_echo;

use errors::JsDebuggerError;
use js_sys::{Error, JsString};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};

use serde::{Serialize, Deserialize};
use serde_json;

use js_witness_map::JsWitnessMap;

#[allow(deprecated)]
use barretenberg_blackbox_solver::BarretenbergSolver;

/**
 * Refactor WasmBlackBoxFunctionSolver related stuff to re-use (copied from acvm-js)
 */
#[wasm_bindgen]
#[allow(deprecated)]
pub struct WasmBlackBoxFunctionSolver(BarretenbergSolver, String);

#[cfg(target_arch = "wasm32")]
impl WasmBlackBoxFunctionSolver {
    async fn initialize() -> WasmBlackBoxFunctionSolver {
        #[allow(deprecated)]
        WasmBlackBoxFunctionSolver(BarretenbergSolver::initialize().await, "hi".to_string())
    }
}

#[wasm_bindgen(js_name = "createBlackBoxSolver")]
#[cfg(target_arch = "wasm32")]
pub async fn create_black_box_solver() -> WasmBlackBoxFunctionSolver {
    WasmBlackBoxFunctionSolver::initialize().await
}
/******/

#[wasm_bindgen(js_name = echo)]
pub fn echo(say: JsString) -> Result<JsString, JsDebuggerError> {
    console_error_panic_hook::set_once();
    Ok(debug_echo(say.into()).into())
}

/// Debugs an ACIR circuit to generate the solved witness from the initial witness.
///
/// @param {&WasmBlackBoxFunctionSolver} solver - A black box solver.
/// @param {Uint8Array} circuit - A serialized representation of an ACIR circuit
/// @param {WitnessMap} initial_witness - The initial witness map defining all of the inputs to `circuit`..
/// @param {ForeignCallHandler} foreign_call_handler - A callback to process any foreign calls from the circuit.
/// @returns {WitnessMap} The solved witness calculated by executing the circuit on the provided inputs.
#[wasm_bindgen(js_name = debugWithSolver, skip_jsdoc)]
#[cfg(target_arch = "wasm32")]
pub fn debug_with_solver(
    solver: &WasmBlackBoxFunctionSolver,
    circuit: Vec<u8>,
    artifact: &str,
    initial_witness: JsWitnessMap,
) -> Result<JsWitnessMap, JsDebuggerError> {
    console_error_panic_hook::set_once();

    let circuit: Circuit =
        Circuit::deserialize_circuit(&circuit).expect("Failed to deserialize circuit");

    // TODO: there's some overlap between this and
    // the compiler/wasm NoirCompilationArtifacts.
    // For now we're sending ContractArtifact's as defined
    // in Aztec's monorrepo foundation ContractArtifact interface
    // but maybe the correct thing to do is to convert that into
    // a NoirCompilationArtifacts type as defined in 
    // noir_artifact.ts inside the noir_compiler folder inside
    // the Aztec js monorrepo.
    // Let's see what plays better with the debugger.
    // In principle, ContractArtifact's may have more than is necessary
    // for the debugger. So probably we should stick to NoirCompilationArtifacts
    #[derive(Serialize, Deserialize)]
    struct Artifact {
        name: String
    }

    let parsed_artifact: Artifact = serde_json::from_str(artifact).map_err(|e| e.to_string())?;

    println!("{:?}", parsed_artifact.name);

    // Witness de-serialization
    let witness: WitnessMap = initial_witness.into();


    Ok(witness.into())
}
