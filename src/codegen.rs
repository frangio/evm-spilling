use std::{collections::HashMap, num::NonZeroUsize};

use alloy_primitives::U256;
use eyre::{ensure, Ok, OptionExt, Result};

use crate::{analysis::count_occurrences, evm::{self, DataInstruction, StackInstruction}, program::{Expression, Statement}, scope::{ResolvedBlock, Var}};

#[derive(PartialEq, Eq, Hash)]
pub struct Label {
    pub index: usize,
}

enum LabeledControlInstruction {
    Jump(Label),
    Jumpi(Label),
    Jumpdest,
}

enum RegisterInstruction {
    Dup(usize),
    Swap(usize),
}

enum Instruction {
    Stack(StackInstruction),
    Control(LabeledControlInstruction),
    Data(DataInstruction),
    Register(RegisterInstruction),
    Spill(Box<(Instruction, RegisterInstruction)>),
}

impl Into<evm::Instruction> for Instruction {
    fn into(self) -> evm::Instruction {
        match self {
            Instruction::Stack(i) => evm::Instruction::Stack(i),
            Instruction::Control(_) => todo!(),
            Instruction::Data(i) => evm::Instruction::Data(i),
            Instruction::Register(_) => todo!(),
            Instruction::Spill(_) => todo!(),
        }
    }
}

#[derive(Clone, Copy)]
enum VarInstance {
    Main(Var),
    Copy(Var),
}

struct VarLocation {
    main_index: usize,
    copy_index_plus1: Option<NonZeroUsize>,
}

impl VarLocation {
    fn new(index: usize) -> VarLocation {
        VarLocation { main_index: index, copy_index_plus1: None }
    }

    fn get_copy_index(&self) -> Option<usize> {
        self.copy_index_plus1.map(|i| i.get() - 1)
    }

    fn set_copy_index(&mut self, i: Option<usize>) {
        self.copy_index_plus1 = i.and_then(|i| NonZeroUsize::new(i + 1))
    }
}

fn set_location(location: &mut HashMap<Var, VarLocation>, instance: VarInstance, index: Option<usize>) {
    match instance {
        VarInstance::Main(name) => {
            if let Some(index) = index {
                location.get_mut(&name).unwrap().main_index = index;
            } else {
                location.remove(&name).unwrap();
            }
        },
        VarInstance::Copy(name) => {
            location.get_mut(&name).unwrap().set_copy_index(index);
        },
    }
}

struct Data {
    stack: Vec<VarInstance>,
    location: HashMap<Var, VarLocation>,
}

impl Data {
    fn new() -> Data {
        Data {
            stack: Vec::new(),
            location: HashMap::new(),
        }
    }

    fn find_depth(&self, name: Var) -> usize {
        let loc = self.location.get(&name).unwrap();
        let index = loc.get_copy_index().unwrap_or(loc.main_index);
        self.stack.len() - 1 - index
    }

    fn pop(&mut self) {
        let instance = self.stack.pop().unwrap();
        set_location(&mut self.location, instance, None);
    }

    fn push(&mut self, name: Var) {
        self.stack.push(VarInstance::Main(name));
        let index = self.stack.len() - 1;
        self.location.insert(name, VarLocation::new(index));
    }

    fn copy(&mut self, name: Var) {
        let instance = VarInstance::Copy(name);
        self.stack.push(instance);
        let index = self.stack.len() - 1;
        set_location(&mut self.location, instance, Some(index));
    }

    fn swap(&mut self, from_depth: usize, to_depth: usize) {
        let top = self.stack.len() - 1;
        let from_index = top - from_depth;
        let to_index = top - to_depth;
        let from_instance = self.stack[from_index];
        let to_instance = self.stack[to_index];
        self.stack.swap(from_index, to_index);
        set_location(&mut self.location, from_instance, Some(to_index));
        set_location(&mut self.location, to_instance, Some(from_index));
    }

    fn drain(&mut self, count: usize) {
        for instance in self.stack.drain(self.stack.len() - count..) {
            set_location(&mut self.location, instance, None);
        }
    }

    fn extend(&mut self, names: &[Var]) {
        let first = self.stack.len();
        let instances = names.iter().enumerate().map(|(i, &name)| {
            self.location.insert(name, VarLocation::new(first + i));
            VarInstance::Main(name)
        });
        self.stack.extend(instances);
    }
}

struct Machine {
    code: Vec<Instruction>,
    data: Data,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            code: Vec::new(),
            data: Data::new(),
        }
    }

    fn pop(&mut self) {
        self.data.pop();
        self.code.push(Instruction::Data(DataInstruction::Pop));
    }

    fn push(&mut self, name: Var, value: U256) {
        self.data.push(name);
        self.code.push(Instruction::Stack(StackInstruction::Push(value.into())));
    }

    fn swap_to(&mut self, from_name: Var, to_depth: usize) -> usize {
        assert!(to_depth <= 16, "Swap too deep");
        let from_depth = self.data.find_depth(from_name);
        if from_depth != to_depth {
            self.data.swap(from_depth, to_depth);
            if from_depth != 0 {
                self.code.push(Instruction::Stack(StackInstruction::Swap(from_depth)));
            }
            if to_depth != 0 {
                self.code.push(Instruction::Stack(StackInstruction::Swap(to_depth)));
            }
        }
        from_depth
    }

    fn copy_to(&mut self, from_name: Var, to_depth: usize) -> usize {
        assert!(to_depth <= 16, "Copy too deep");
        let from_depth = self.data.find_depth(from_name);
        self.data.copy(from_name);
        self.code.push(Instruction::Stack(StackInstruction::Dup(from_depth)));
        if to_depth != 0 {
            self.data.swap(0, to_depth);
            if to_depth != from_depth + 1 {
                self.code.push(Instruction::Stack(StackInstruction::Swap(to_depth)));
            }
        }
        from_depth
    }

    fn apply(&mut self, op: DataInstruction, ress: &[Var]) {
        let (nargs, _) = op.arity();
        self.data.drain(nargs);
        self.data.extend(ress);
        self.code.push(Instruction::Data(op));
    }
}

pub fn generate(rblock: &ResolvedBlock) -> Result<impl Iterator<Item=impl Into<evm::Instruction>>> {
    let mut occurs = count_occurrences(&rblock);
    let mut machine = Machine::new();

    for Statement(ress, e) in &rblock.block.0 {
        match *e {
            Expression::Const(c) => {
                ensure!(ress.len() == 1, "Wrong number of results");
                let name = ress[0];
                machine.push(name, c);
            },

            Expression::Op(ref op, ref args) => {
                let op: DataInstruction = op.parse()?;
                let (nargs, nres) = op.arity();

                ensure!(args.len() == nargs, "Wrong number of arguments");
                ensure!(ress.len() == nres, "Wrong number of results");

                let mut ndups = 0;
                let dups: Vec<_> = args.iter()
                    .map(|&a| {
                        occurs[a.index()] -= 1;
                        let d = occurs[a.index()] > 0;
                        if d { ndups += 1; }
                        d
                    })
                    .collect();

                for (i, (&arg, dup)) in args.iter().zip(dups).enumerate().rev() {
                    if dup {
                        ndups -= 1;
                    }
                    let to_depth = i - ndups;
                    if dup {
                        machine.copy_to(arg, to_depth);
                    } else {
                        machine.swap_to(arg, to_depth);
                    }
                }

                machine.apply(op, ress);
            },
        }

        for &r in ress.iter().rev() {
            if occurs[r.index()] == 0 {
                machine.swap_to(r, 0);
                machine.pop();
            }
        }
    }


    Ok(machine.code.into_iter())
}
