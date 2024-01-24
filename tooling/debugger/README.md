# Noir Debugger

There are currently two ways of debugging Noir programs, both in active development and in experimental phase:

1. The REPL debugger, which currently ships with Nargo behind a feature flag.
2. The VS Code extension, which hasn't still reached minimum viability, and so must be manually set up.

This README explains how to use each of them as well as specifying which features are currently mature and which ones are unstable.

## Supported project types

At the time of writing, the debugger supports debugging binary projects, but not contracts. At the end of this README, we'll elaborate on what the current state of Noir contract debugging is, and the pre-requisites to fulfil.


## REPL debugger

In order to use the REPL debugger, you will need to install a new enough version of Nargo. At the time of writing, the nightly version is 0.20.0, so we'll base this guide on it.

Let's debug a simple circuit:

```
fn main(x : Field, y : pub Field) {
    assert(x != y);
}
```

To start the REPL debugger, using a terminal, go to a Noir circuit's home directory. Then:

`$ nargo debug`

You should be seeing this in your terminal:

```
[main] Starting debugger
At opcode 0: EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
At ~/noir-examples/recursion/circuits/main/src/main.nr:2:12
  1    fn main(x : Field, y : pub Field) {
  2 ->     assert(x != y);
  3    }
> 
```

The debugger displays the current opcode, and the corresponding Noir code location associated to it, and it is now waiting for us to drive it.

Let's first take a look at the available commands. For that we'll use the `help` command.

```
At ~/noir-examples/recursion/circuits/main/src/main.nr:2:12
  1    fn main(x : Field, y : pub Field) {
  2 ->     assert(x != y);
  3    }
> help
Available commands:

  break LOCATION:OpcodeLocation    add a breakpoint at an opcode location
  memory                           show Brillig memory (valid when executing a
                                   Brillig block)
  acir-next                        execute until the next ACIR opcode, skipping Brilling blocks
  next                             step until a new source location is reached
  delete LOCATION:OpcodeLocation   delete breakpoint at an opcode location
  step                             step into the next ACIR or Brillig opcode
  registers                        show Brillig registers (valid when executing
                                   a Brillig block)
  regset index:usize value:String  update a Brillig register with the given
                                   value
  restart                          restart the debugging session
  witness                          show witness map
  witness index:u32                display a single witness from the witness map
  witness index:u32 value:String   update a witness with the given value
  continue                         continue execution until the end of the
                                   program
  opcodes                          display ACIR opcodes
  memset index:usize value:String  update a Brillig memory cell with the given
                                   value

Other commands:

  help  Show this help message
  quit  Quit repl
```

The command menu is pretty self-explanatory. Some commands operate only at Brillig level, such as `memory`, `memset`, `registers`, `regset`. If you try to use them while execution is paused at an ACIR opcode, the debugger will simply inform you that you are not executing Brillig code:

```
> registers
Not executing a Brillig block
> 
```

Before continuing, we can take a look at the initial witness map:

```
> witness
_1 = 1
_2 = 2
> 
```

Cool, since `x==1`, `y==2`, and we want to check that `x != y`, our circuit should succeed. At this point we could intervene and use the witness setter command to change one of the witnesses. Let's set `y=3`, then back to 2:

```
> witness
_1 = 1
_2 = 2
> witness 2 3
_2 = 3
> witness
_1 = 1
_2 = 3
> witness 2 2
_2 = 2
> witness
_1 = 1
_2 = 2
> 
```

Let's take a look at this circuit's ACIR, using the `opcodes` command:

```
> opcodes
  0 -> EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1    BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |   JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |   BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2    EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> 
```

Note: in future versions of the debugger, we could explore prettier or more compact formats to print opcodes.

So the next opcode will take us to Brillig execution. Let's step into opcode 1 so we can explore Brillig debugger commands.

```
> step
At opcode 1: BRILLIG: inputs: [Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
outputs: [Simple(Witness(4))]
[JumpIfNot { condition: RegisterIndex(0), location: 3 }, Const { destination: RegisterIndex(1), value: Value { inner: 1 } }, BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }, Stop]

At /~/noir-examples/recursion/circuits/main/src/main.nr:2:12
  1    fn main(x : Field, y : pub Field) {
  2 ->     assert(x != y);
  3    }
```

In disassembly view:

```
> op
  0    EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1 -> BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |-> JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |   BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2    EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> witness
_1 = 1
_2 = 2
_3 = 1
> 
```

We can see two arrow `->` cursors: one indicates where we are from the perspective of ACIR (opcode 1), and the other one shows us where we are in the context of the current Brillig block (opcode 1.0).

Note: REPL commands are autocompleted when not ambiguous, so `opcodes` can be run just with `op`, `step` with `s`, etc.

The next opcode to execute is a `JumpIfNot`, which reads from register 0. Let's inspect Brillig register state:

```
> op
  0    EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1 -> BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |-> JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |   BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2    EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> registers
Brillig VM registers not available
```

Oops. This is unexpected, even though we were already in a Brillig block, we couldn't access the Brillig registers. This is a known issue: when just entering the Brillig block, the ACVM has not yet initialized the Brillig VM, so we can't introspect it.

Note: In order to solve this, we would have to change the way the ACVM works, or add special handling for this case (after all, the debugger does know we're at the first opcode of a Brillig block and could keep track of how registers will be initialized). At the time of writing, we haven't yet solved this case. 

For now, let's just step once more:

```
> i
At opcode 1.1: Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
> registers
0 = 1
>
```

Now we can see that register 0 was initialized with a value of 1, so the `JumpIfNot` didn't activate. After executing opcode 1, we should see register 1 gets a value of 1:

```
> i
At opcode 1.2: BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
> regist
0 = 1
1 = 1
>
```

The last operation will compute `Reg0 <- Reg1 / Reg0`: 

```
> i
At opcode 1.3: Stop
> registers
0 = 1
1 = 1
>
```

Once we step again, we'll be out of Brillig and back on ACVM territory. With a new witness `_4` corresponding to the result of the Brillig block execution:

```
> op
  0    EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1    BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |   JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |   BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2 -> EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> wit
_1 = 1
_2 = 2
_3 = 1
_4 = 1
>
```

At any time, we might also decide to restart from the beginning:

```
> restart
Restarted debugging session.
At opcode 0: EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
At ~/noir-examples/recursion/circuits/main/src/main.nr:2:12
  1    fn main(x : Field, y : pub Field) {
  2 ->     assert(x != y);
  3    }
>
```

Let's set a breakpoint. For that, we can use the opcode id's listed by the `opcodes` command:

```
> opcodes
  0 -> EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1    BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |   JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |   BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2    EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> break 1.2
Added breakpoint at opcode 1.2
```

Now we can have the debugger continue all the way to opcode 1.2:

```
> break 1.2
Added breakpoint at opcode 1.2
> continue
(Continuing execution...)
Stopped at breakpoint in opcode 1.2
At opcode 1.2: BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
> opcodes
  0    EXPR [ (-1, _1) (1, _2) (-1, _3) 0 ]
  1 -> BRILLIG inputs=[Single(Expression { mul_terms: [], linear_combinations: [(1, Witness(3))], q_c: 0 })]
       |       outputs=[Simple(Witness(4))]
  1.0  |   JumpIfNot { condition: RegisterIndex(0), location: 3 }
  1.1  |   Const { destination: RegisterIndex(1), value: Value { inner: 1 } }
  1.2  |-> BinaryFieldOp { destination: RegisterIndex(0), op: Div, lhs: RegisterIndex(1), rhs: RegisterIndex(0) }
  1.3  |   Stop
  2    EXPR [ (1, _3, _4) (1, _5) -1 ]
  3    EXPR [ (1, _3, _5) 0 ]
  4    EXPR [ (-1, _5) 0 ]
> 
```

Let's continue to the end:

```
> continue
(Continuing execution...)
Finished execution
> q
[main] Circuit witness successfully solved
```

Upon quitting the debugger after a solved circuit, the resulting circuit witness gets saved, equivalent to what would happen if we had run the same circuit with `nargo execute`.


## vars

Running `vars` will print the current variables in scope, and its current values:

```
At /mul_1/src/main.nr:6:5
  1    // Test unsafe integer multiplication with overflow: 12^8 = 429 981 696
  2    // The circuit should handle properly the growth of the bit size
  3    fn main(mut x: u32, y: u32, z: u32) {
  4        x *= y;
  5        x *= x; //144
  6 ->     x *= x; //20736
  7        x *= x; //429 981 696
  8        assert(x == z);
  9    }
> vars
y:UnsignedInteger { width: 32 }=Field(4), z:UnsignedInteger { width: 32 }=Field(2¹⁶×6561), x:UnsignedInteger { width: 32 }=Field(2⁴×9)
> 
```

## stacktrace

Running `stacktrace` will print information about the current frame in the stacktrace:

```
> stacktrace
Frame #0, opcode 12: EXPR [ (1, _5, _5) (-1, _6) 0 ]
At /1_mul/src/main.nr:6:5
  1    // Test unsafe integer multiplication with overflow: 12^8 = 429 981 696
  2    // The circuit should handle properly the growth of the bit size
  3    fn main(mut x: u32, y: u32, z: u32) {
  4        x *= y;
  5        x *= x; //144
  6 ->     x *= x; //20736
  7        x *= x; //429 981 696
  8        assert(x == z);
  9    }
> 
```


## Towards debugging contracts

### Contracts Runtime

The execution of Noir contracts depends on a foreign call execution runtime to resolve all the oracle calls that the contract functions depend on.

This means for the debugger to be usable with contracts we need to be able to do at least one of the following:

1. Let users mock out a foreign call executor, and run the debugger with it.
2. Instrument live environments, such as the Sandbox, so that calls and transactions can be driven by the debugger, which ultimately means the debugger would use the same foreign call executor a live Sandbox uses for normal execution of Noir circuits.

Both of these scenarios imply making the debugger available to language runtimes external to Noir. The Sandbox/PXE runs on JS runtimes, and an hypothetical mockable foreign executor could be in principle written in any language. So it seems the most promising way forward is to make sure that the debugger itself is consumable in JS runtimes.

