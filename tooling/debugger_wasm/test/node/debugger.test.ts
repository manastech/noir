import { expect } from 'chai';
import {
  echo,
  debugWithSolver,
  WasmBlackBoxFunctionSolver,
  createBlackBoxSolver
} from '@noir-lang/debugger';
import CounterJson from './Counter.json';


/**
 * Given an artifact it looks for its bytecode
 * in an artifact and returns it.
 * 
 * Based on aztec-cli/src/utils.ts#getFunctionArtifact
 * 
 * At the moment I prefer not to introduce a dependency.
 * We should either extract these conveniences to some leaner lib,
 * or make sure that they're covered by integration tests.
 * 
function getCounterArtifact(
  artifact: object,
  functionName: string,
  owner: string,
): object {
  // First we retrieve the function by name
  const fn = artifact.functions.find(({ name }) => name === functionName);
    if (!fn) {
    throw Error(`Function ${fnName} not found in contract ABI.`);
  }

  const ownerAsBigInt = BigInt(owner);

  // Currently from isn't used in PXE service (?)
  // const from = "0x16efad912187aa8ef0dcc6ef4f3743ab327b06465d4d229943f2fe3f88b06ad9";
  // Extracted from AztecAddress
  //const fromBuffer = Buffer.from(address.replace(/^0x/i, ''), 'hex');

  const args = encodeArguments(fn)


  return fn;
}
 */

it('calls echo', async () => {
  expect(echo("hello")).to.equal("hello");
});

it('successfully passes debug artifact to debugger', async function () {
  const solver: WasmBlackBoxFunctionSolver = await createBlackBoxSolver();

  const { bytecode, expectedWitnessMap } = await import('./pedersen');

  const initialWitnessMap = new Map([[1, '0x16efad912187aa8ef0dcc6ef4f3743ab327b06465d4d229943f2fe3f88b06ad9']]);

  const getCounterFn = CounterJson.functions.find(f => f.name === 'get_counter');
  if (!getCounterFn) throw new Error('could not find get_counter in Counter.json artifact');
  const getCounterBytecode = Buffer.from(getCounterFn.bytecode, 'base64');

  const result = debugWithSolver(
    solver,
    getCounterBytecode,
    JSON.stringify({
      debug_symbols: CounterJson.debug.debugSymbols,
      file_map: CounterJson.debug.fileMap,
    }),
    initialWitnessMap
  );

  expect(result).to.be.deep.eq(expectedWitnessMap);
});
