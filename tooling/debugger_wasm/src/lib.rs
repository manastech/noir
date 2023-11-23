#![warn(unused_crate_dependencies, unused_extern_crates)]
#![warn(unreachable_pub)]
#![warn(clippy::semicolon_if_nothing_returned)]

mod errors;
mod js_witness_map;
mod foreign_call;

use acvm::{
    acir::{
        circuit::Circuit,
        native_types::WitnessMap,
    },
    pwg::{ACVMStatus, ErrorLocation, OpcodeResolutionError, ACVM},
};

// See Cargo.toml for explanation.
use getrandom as _;

use noir_debugger::{
    debug_echo,
    context::{DebugCommandResult, DebugContext},
};

use errors::JsDebuggerError;
use js_sys::{Error, JsString};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};

use serde::{Serialize, Deserialize};
use serde_json;

use js_witness_map::JsWitnessMap;

#[allow(deprecated)]
use barretenberg_blackbox_solver::BarretenbergSolver;

use nargo::artifacts::debug::DebugArtifact;

use std::{
    io::Read,
    collections::BTreeMap,
};
use base64::{decode, DecodeError};
use flate2::read::ZlibDecoder;

use noirc_errors::{debug_info::DebugInfo, Location};

use fm::{FileId, FileManager, PathString};

use noirc_driver::{CompiledContract, CompiledProgram, DebugFile};

use acvm::{
    acir::{BlackBoxFunc, FieldElement},
    BlackBoxFunctionSolver
};

use crate::{
    foreign_call::{resolve_brillig, ForeignCallHandler},
};

fn decode_base64_symbols(base64_symbols: Vec<String>) -> Result<Vec<String>, JsDebuggerError> {
    let mut decoded_symbols = Vec::with_capacity(base64_symbols.len());

    for base64_symbol in base64_symbols {
        let gzipped_symbol = decode(&base64_symbol).map_err(|_| JsDebuggerError::new("Not a base64 string".to_string()))?;
        let mut gz = ZlibDecoder::new(&gzipped_symbol[..]);
        let mut inflated_symbol = Vec::new();
        gz.read_to_end(&mut inflated_symbol).map_err(|e| JsDebuggerError::new(format!("Failed to inflate symbol: {}", e)))?;

        let decoded_symbol = String::from_utf8(inflated_symbol).map_err(|_| JsDebuggerError::new("Decoded text is not valid UTF-8".to_string()))?;

        decoded_symbols.push(decoded_symbol);
    }

    Ok(decoded_symbols)
}


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

#[allow(deprecated)]
impl BlackBoxFunctionSolver for WasmBlackBoxFunctionSolver {
    fn schnorr_verify(
        &self,
        public_key_x: &FieldElement,
        public_key_y: &FieldElement,
        signature: &[u8],
        message: &[u8],
    ) -> Result<bool, BlackBoxResolutionError> {
        self.0.schnorr_verify(public_key_x, public_key_y, signature, message)
    }

    fn pedersen_commitment(
        &self,
        inputs: &[FieldElement],
        domain_separator: u32,
    ) -> Result<(FieldElement, FieldElement), BlackBoxResolutionError> {
        self.0.pedersen_commitment(inputs, domain_separator)
    }

    fn pedersen_hash(
        &self,
        inputs: &[FieldElement],
        domain_separator: u32,
    ) -> Result<FieldElement, BlackBoxResolutionError> {
        self.0.pedersen_hash(inputs, domain_separator)
    }

    fn fixed_base_scalar_mul(
        &self,
        low: &FieldElement,
        high: &FieldElement,
    ) -> Result<(FieldElement, FieldElement), BlackBoxResolutionError> {
        self.0.fixed_base_scalar_mul(low, high)
    }
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
    foreign_call_handler: ForeignCallHandler,
) -> Result<JsString, JsDebuggerError> {
    console_error_panic_hook::set_once();

    let circuit: Circuit =
        Circuit::deserialize_circuit(&circuit).expect("Failed to deserialize circuit");


    #[derive(Serialize, Deserialize)]
    struct Artifact {
        debug_symbols: Vec<String>,
        file_map: BTreeMap<FileId, DebugFile>
    }

    let parsed_artifact: Artifact = serde_json::from_str(artifact).map_err(|e| format!("Failed parsing artifact {}", e))?;
    let base64_debug_symbols: Vec<String> = parsed_artifact.debug_symbols;
    let debug_symbols: Vec<String> = decode_base64_symbols(base64_debug_symbols)?;
    let parsed_debug_infos: Result<Vec<DebugInfo>, serde_json::Error> = 
        debug_symbols.into_iter()
            .map(|s| serde_json::from_str(&s)).collect();
    let debug_infos: Vec<DebugInfo> = parsed_debug_infos.map_err(|e| format!("Failed parsing debug symbols {}", e))?;

    // Witness deserialization
    let witness: WitnessMap = initial_witness.into();

    let debug_artifact = DebugArtifact {
        debug_symbols: debug_infos.clone(),
        file_map: parsed_artifact.file_map.clone(),
        warnings: vec![], // Contract artifacts aren't persisting warnings
    };

    let mut context = DebugContext::new_with_foreign_call_executor(solver, &circuit, &debug_artifact, witness.clone(), foreign_call_handler);
    context.cont();

    if context.is_solved() {
        let solved_witness = context.finalize();
        Ok("Witness solved!".into())
    } else {
        Ok("Witness NOT solved ;(!".into())
    }
}
