import { expect } from 'chai';
import {
  echo,
  debugWithSolver,
  WasmBlackBoxFunctionSolver,
  createBlackBoxSolver
} from '@noir-lang/debugger';
import CounterJson from './Counter.json';

it('calls echo', async () => {
  expect(echo("hello")).to.equal("hello");
});

it('successfully passes debug artifact to debugger', async function () {
  const solver: WasmBlackBoxFunctionSolver = await createBlackBoxSolver();

  const { bytecode } = await import('./pedersen');

  const initialWitnessMap = new Map([[1, '0x16efad912187aa8ef0dcc6ef4f3743ab327b06465d4d229943f2fe3f88b06ad9']]);

  const result = debugWithSolver(
    solver,
    bytecode,
    JSON.stringify(CounterJson),
    initialWitnessMap
  );

  expect(result).to.be.deep.eq(initialWitnessMap);
});
