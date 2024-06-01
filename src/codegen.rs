use std::iter::repeat;
use std::collections::HashMap;

use alloy_primitives::U256;
use eyre::{ensure, Ok, Result};

use crate::scope::{ResolvedBlock, Var};
use crate::program::{Expression, Statement};
use crate::evm::{Instruction, DataInstruction, StackInstruction};
use crate::analysis::count_occurrences;

#[derive(Clone)]
enum PreStackInstruction {
    Rotate { from_depth: usize, to_depth: usize },
    Dup(usize),
    Push(Box<U256>),
}

#[derive(Clone)]
enum PreInstruction {
    Stack(PreStackInstruction),
    Data(DataInstruction),
}

#[derive(Clone, Copy)]
enum VarInstance {
    Main(Var),
    Copy(Var),
}

struct VarMeta {
    main_index: usize,
    copy_index: Option<usize>,
}

struct Machine {
    code: Vec<PreInstruction>,
    stack: Vec<VarInstance>,
    meta: HashMap<Var, VarMeta>,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            code: Vec::new(),
            stack: Vec::new(),
            meta: HashMap::new(),
        }
    }

    fn get_meta(&mut self, name: Var) -> &mut VarMeta {
        self.meta.get_mut(&name).unwrap()
    }

    fn set_location(&mut self, instance: VarInstance, index: Option<usize>) {
        match instance {
            VarInstance::Main(name) => {
                if let Some(index) = index {
                    self.get_meta(name).main_index = index;
                } else {
                    let meta = self.meta.remove(&name).unwrap();
                    assert!(meta.copy_index.is_none());
                }
            }

            VarInstance::Copy(name) => {
                self.get_meta(name).copy_index = index;
            }
        }
    }

    fn find(&self, name: Var) -> usize {
        let meta = self.meta.get(&name).unwrap();
        let index = meta.copy_index.unwrap_or(meta.main_index);
        let depth = self.stack.len() - 1 - index;
        depth
    }

    fn pop(&mut self) {
        let instance = self.stack.pop().unwrap();
        self.set_location(instance, None);
        self.code.push(PreInstruction::Stack(PreStackInstruction::Rotate { from_depth: 0, to_depth: 0 }));
        self.code.push(PreInstruction::Data(DataInstruction::Pop));
    }

    fn push(&mut self, name: Var, value: U256) {
        self.stack.push(VarInstance::Main(name));
        self.meta.insert(name, VarMeta {
            main_index: self.stack.len() - 1,
            copy_index: None,
        });
        self.code.push(PreInstruction::Stack(PreStackInstruction::Push(value.into())));
    }

    fn stack_swap(&mut self, from_depth: usize, to_depth: usize) {
        let top_index = self.stack.len() - 1;
        let from_index = top_index - from_depth;
        let to_index = top_index - to_depth;
        let from_instance = self.stack[from_index];
        let to_instance = self.stack[to_index];
        self.stack.swap(from_index, to_index);
        self.set_location(from_instance, Some(to_index));
        self.set_location(to_instance, Some(from_index));
    }

    fn rotate_to(&mut self, from_name: Var, to_depth: usize) {
        assert!(to_depth <= 16, "Swap too deep");
        let from_depth = self.find(from_name);
        self.stack_swap(from_depth, 0);
        self.stack_swap(0, to_depth);
        self.code.push(PreInstruction::Stack(PreStackInstruction::Rotate { from_depth, to_depth }));
    }

    fn copy_to(&mut self, from_name: Var, to_depth: usize) {
        assert!(to_depth <= 16, "Copy too deep");

        let from_depth = self.find(from_name);
        let copy_instance = VarInstance::Copy(from_name);
        self.stack.push(copy_instance);
        self.set_location(copy_instance, Some(self.stack.len() - 1));
        self.code.push(PreInstruction::Stack(PreStackInstruction::Dup(from_depth)));

        if to_depth != 0 {
            self.stack_swap(0, to_depth);
            self.code.push(PreInstruction::Stack(PreStackInstruction::Rotate { from_depth: 0, to_depth }));
        }
    }

    fn apply(&mut self, op: DataInstruction, ress: &[Var]) {
        let (nargs, nress) = op.arity();
        let stack_base = self.stack.len() - nargs;

        let removed = self.stack.split_off(stack_base);
        for &instance in &removed {
            self.set_location(instance, None);
        }
        self.stack.extend(ress.iter().map(|&name| VarInstance::Main(name)));

        self.code.push(PreInstruction::Data(op));

        for (i, &name) in ress.iter().enumerate() {
            self.meta.insert(name, VarMeta {
                main_index: stack_base + i,
                copy_index: None,
            });
        }
    }
}

#[derive(Clone, Copy)]
struct SpillLocation {
    code_index: usize,
    depth: usize,
}

struct Spill {
    location: SpillLocation,
    outward: bool,
}

fn make_spills(machine: &Machine) -> Vec<Spill> {
    enum SpillStatus {
        Unspillable,
        MaybeSpilled(SpillLocation),
        Spilled,
        MaybeRestored(SpillLocation),
        Restored,
    }

    use SpillStatus::*;

    impl SpillStatus {
        fn set_reachable_at(&mut self, location: SpillLocation) {
            assert!(location.depth < 16);
            match *self {
                Unspillable => (),
                MaybeSpilled(_) => *self = MaybeSpilled(location),
                Spilled => *self = MaybeRestored(location),
                MaybeRestored(_) => (),
                Restored => panic!("already restored?"),
            }
        }
    }

    struct State {
        stack: Vec<SpillStatus>,
        spills: Vec<Spill>,
    }

    impl State {
        fn ensure_reachable(&mut self, depth: usize) {
            if depth >= 16 {
                let index = self.stack.len() - 1 - depth;
                let ref mut status = self.stack[index];
                match *status {
                    Unspillable => panic!("unspillable accessed too deep"),
                    MaybeSpilled(l) => {
                        *status = Spilled;
                        self.spills.push(Spill { location: l, outward: true });
                    }
                    Spilled => (),
                    MaybeRestored(_) => *status = Spilled,
                    Restored => panic!("restored back too deep"),
                }
            }
        }
    }

    let mut state = State {
        stack: Vec::with_capacity(machine.stack.capacity()),
        spills: Vec::new(),
    };

    for (code_index, instr) in machine.code.iter().enumerate() {
        match *instr {
            PreInstruction::Stack(PreStackInstruction::Rotate { from_depth, to_depth }) => {
                assert!(to_depth < 16);

                let top_index = state.stack.len() - 1;
                let from_index = top_index - from_depth;
                let to_index = top_index - to_depth;

                state.ensure_reachable(from_depth);

                if from_depth < 16 {
                    state.stack[from_index].set_reachable_at(SpillLocation { code_index, depth: to_depth });
                } else {
                    assert!(!matches!(state.stack[top_index], Unspillable));
                    state.stack[top_index] = Spilled;

                    assert!(matches!(state.stack[from_index], Spilled));
                    state.stack[from_index] = Restored;
                }

                state.stack.swap(from_index, top_index);
                state.stack.swap(top_index, to_index);
            }

            PreInstruction::Stack(PreStackInstruction::Dup(depth)) => {
                state.ensure_reachable(depth);
                if depth + 1 < 16 {
                    let index = state.stack.len() - 1 - depth;
                    state.stack[index].set_reachable_at(SpillLocation { code_index, depth: depth + 1 });
                }
                state.stack.push(Unspillable);
            }

            PreInstruction::Stack(PreStackInstruction::Push(_)) => {
                state.stack.push(MaybeSpilled(SpillLocation { code_index, depth: 0 }));
            }

            PreInstruction::Data(op) => {
                let (nargs, nress) = op.arity();
                for status in state.stack.drain(state.stack.len() - nargs..) {
                    if let MaybeRestored(l) = status {
                        state.spills.push(Spill { location: l, outward: false });
                    } else if let Spilled = status {
                        panic!("spilled value not restored");
                    }
                }
                state.stack.extend((0..nress).rev().map(|depth|
                    MaybeSpilled(SpillLocation { code_index, depth })
                ));
            }
        }
    }

    state.spills.sort_unstable_by_key(|s| s.location.code_index);
    state.spills
}

fn register_store(register: usize) -> impl Iterator<Item=Instruction> {
    use Instruction::*;
    use StackInstruction::*;
    use DataInstruction::*;

    let ptr = (register * 32).try_into().unwrap();
    [
        Stack(Push(Box::new(ptr))),
        Data(Mstore),
    ].into_iter()
}

fn register_load(register: usize) -> impl Iterator<Item=Instruction> {
    use Instruction::*;
    use StackInstruction::*;
    use DataInstruction::*;

    let ptr = (register * 32).try_into().unwrap();
    [
        Stack(Push(Box::new(ptr))), // todo: fix register location
        Data(Mload),
    ].into_iter()
}

pub fn generate(rblock: &ResolvedBlock) -> Result<impl Iterator<Item=Instruction>> {
    let mut occurs = count_occurrences(&rblock);
    let mut machine = Machine::new();

    for Statement(ress, e) in &rblock.block.0 {
        match *e {
            Expression::Const(c) => {
                ensure!(ress.len() == 1, "Wrong number of results");
                let name = ress[0];
                machine.push(name, c);
            }

            Expression::Op(ref op, ref args) => {
                let op: DataInstruction = op.parse()?;
                let (nargs, nres) = op.arity();

                ensure!(args.len() == nargs, "Wrong number of arguments");
                ensure!(ress.len() == nres, "Wrong number of results");

                let mut ndups = 0;
                let dups: Vec<_> = args.iter()
                    .map(|&a| {
                        occurs[a.index()] -= 1;
                        let dup = occurs[a.index()] > 0;
                        if dup { ndups += 1; }
                        dup
                    })
                    .collect();

                for (i, (&arg, dup)) in args.iter().zip(dups).enumerate().rev() {
                    if dup { ndups -= 1; }
                    let to_depth = i - ndups;
                    if dup {
                        machine.copy_to(arg, to_depth);
                    } else {
                        machine.rotate_to(arg, to_depth);
                    }
                }

                machine.apply(op, ress);
            }
        }

        for &r in ress.iter().rev() {
            if occurs[r.index()] == 0 {
                machine.rotate_to(r, 0);
                machine.pop();
            }
        }
    }

    let spills = make_spills(&machine);

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StackItem {
        Stack { value: usize },
        Register { register: usize }
    }

    let mut code = Vec::with_capacity(machine.code.capacity());
    let mut stack: Vec<Option<usize>> = Vec::with_capacity(machine.stack.capacity());
    let mut register_count = 0;
    let mut free_registers = Vec::new();

    let mut spills_end = 0;

    for (code_index, instr) in machine.code.into_iter().enumerate() {
        let spills_start = spills[spills_end..].iter()
            .position(|s| s.location.code_index >= code_index)
            .map_or(spills.len(), |i| i + spills_end);

        spills_end = spills[spills_start..].iter()
            .position(|s| s.location.code_index > code_index)
            .map_or(spills.len(), |i| i + spills_start);

        let instr_spills = &spills[spills_start..spills_end];

        match instr {
            PreInstruction::Stack(PreStackInstruction::Rotate { from_depth, to_depth }) if from_depth != to_depth => {
                let top_index = stack.len() - 1;
                let from_index = top_index - from_depth;
                let to_index = top_index - to_depth;

                if from_depth < 16 {
                    if from_depth > 0 {
                        code.push(Instruction::Stack(StackInstruction::Swap(from_depth)));
                        stack.swap(from_index, top_index);
                    }
                } else {
                    let from_register = stack[from_index].unwrap();
                    code.extend(register_load(from_register));
                    code.push(Instruction::Stack(StackInstruction::Swap(1)));
                    if from_depth != 0 {
                        if let Some(top_register) = stack[top_index].take() {
                            free_registers.push(top_register);
                            code.extend(register_load(top_register));
                            code.push(Instruction::Stack(StackInstruction::Swap(1)));
                            code.extend(register_store(top_register));
                        }
                    }
                    code.extend(register_store(from_register));
                }

                if to_depth > 0 {
                    code.push(Instruction::Stack(StackInstruction::Swap(to_depth)));
                    stack.swap(top_index, to_index);
                }
                // todo: more efficient spilling
            }

            PreInstruction::Stack(PreStackInstruction::Rotate { from_depth, to_depth }) => {
                assert_eq!(from_depth, to_depth);
            }

            PreInstruction::Stack(PreStackInstruction::Dup(depth)) => {
                let index = stack.len() - 1 - depth;
                if let Some(register) = stack[index] {
                    code.extend(register_load(register));
                } else {
                    assert!(depth < 16);
                    code.push(Instruction::Stack(StackInstruction::Dup(depth)));
                }
                stack.push(None);
            }

            PreInstruction::Stack(PreStackInstruction::Push(c)) => {
                code.push(Instruction::Stack(StackInstruction::Push(c)));
                stack.push(None);
            }

            PreInstruction::Data(op) => {
                code.push(Instruction::Data(op));
                let (nargs, nress) = op.arity();
                for item in stack.drain(stack.len() - nargs..) {
                    assert!(item.is_none());
                }
                stack.extend(repeat(None).take(nress));
            }
        }

        for &Spill { location, outward } in instr_spills {
            let index = stack.len() - 1 - location.depth;

            let register =
                if outward {
                    assert!(stack[index].is_none());
                    let register = free_registers.pop().unwrap_or_else(|| {
                        let register = register_count;
                        register_count += 1;
                        register
                    });
                    stack[index] = Some(register);
                    register
                } else {
                    let register = stack[index].take().unwrap();
                    free_registers.push(register);
                    register
                };

            code.extend(register_load(register));
            code.push(Instruction::Stack(StackInstruction::Swap(location.depth + 1)));
            code.extend(register_store(register));
        }
    }

    Ok(code.into_iter())
}
