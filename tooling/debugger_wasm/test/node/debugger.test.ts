import { expect } from 'chai';
import {
  echo,
  debugWithSolver,
  WasmBlackBoxFunctionSolver,
  createBlackBoxSolver
} from '@noir-lang/debugger';

it('calls echo', async () => {
  expect(echo("hello")).to.equal("hello");
});

it('successfully passes solver to debugger', async function () {
  const solver: WasmBlackBoxFunctionSolver = await createBlackBoxSolver();

  const { bytecode } = await import('./pedersen');

  const result = debugWithSolver(solver, bytecode);
  expect(result).to.equal(4);
});
