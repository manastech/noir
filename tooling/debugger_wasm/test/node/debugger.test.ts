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
  const result = debugWithSolver(solver);
  expect(result).to.equal("hi");
});