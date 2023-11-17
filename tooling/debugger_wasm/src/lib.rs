#![warn(unused_crate_dependencies, unused_extern_crates)]
#![warn(unreachable_pub)]
#![warn(clippy::semicolon_if_nothing_returned)]

mod errors;

use acvm::{
    acir::circuit::Circuit,
    pwg::{ACVMStatus, ErrorLocation, OpcodeResolutionError, ACVM},
};

// See Cargo.toml for explanation.
use getrandom as _;

use noir_debugger::debug_echo;

use errors::JsDebuggerError;
use js_sys::{Error, JsString};
use wasm_bindgen::{prelude::wasm_bindgen, JsValue};

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
) -> Result<JsString, JsDebuggerError> {
    console_error_panic_hook::set_once();
    let circuit: Circuit =
        Circuit::deserialize_circuit(&circuit).expect("Failed to deserialize circuit");

    Ok(circuit.current_witness_index.to_string().into())
}
