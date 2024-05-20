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

impl VarInstance {
    fn name(&self) -> Var {
        use VarInstance::*;
        match self {
            Main(name) | Copy(name) => *name
        }
    }
}

#[derive(PartialEq, Eq)]
enum SpillStatus {
    Unopened,
    Opened,
    Closed,
}

#[derive(Clone, Copy)]
struct Spill {
    code_index: usize,
    stack_depth: usize,
}

impl Spill {
    fn new(code: &Vec<Instruction>, stack_depth: usize) -> Spill {
        Spill {
            code_index: code.len() - 1,
            stack_depth,
        }
    }
}

struct VarMeta {
    main_index: usize,
    copy_index: Option<usize>,
    spill_status: SpillStatus,
    spill: Spill,
}

struct Machine {
    code: Vec<Instruction>,
    stack: Vec<VarInstance>,
    meta: HashMap<Var, VarMeta>,
    spills: Vec<(Spill, bool)>,
}

impl Machine {
    fn new() -> Machine {
        Machine {
            code: Vec::new(),
            stack: Vec::new(),
            meta: HashMap::new(),
            spills: Vec::new(),
        }
    }

    fn set_location(&mut self, instance: VarInstance, index: Option<usize>) {
        match instance {
            VarInstance::Main(name) => {
                if let Some(index) = index {
                    self.meta.get_mut(&name).unwrap().main_index = index;
                } else {
                    let meta = self.meta.remove(&name).unwrap();
                    if meta.spill_status != SpillStatus::Unopened {
                        self.spills.push((meta.spill, false));
                    }
                }
            },
            VarInstance::Copy(name) => {
                self.meta.get_mut(&name).unwrap().copy_index = index;
            },
        }
    }

    fn set_spill(&mut self, from_depth: usize, name: Var, spill: Spill) {
        let meta = self.meta.get_mut(&name).unwrap();
        if from_depth > 16 {
            if meta.spill_status == SpillStatus::Unopened {
                self.spills.push((meta.spill, true));
            }
            meta.spill_status = SpillStatus::Opened;
            meta.spill = spill;
        } else if meta.spill_status != SpillStatus::Closed {
            meta.spill_status = SpillStatus::Closed;
            meta.spill = spill;
        }
    }

    fn find_depth(&self, name: Var) -> usize {
        let meta = self.meta.get(&name).unwrap();
        let index = meta.copy_index.unwrap_or(meta.main_index);
        self.stack.len() - 1 - index
    }

    fn pop(&mut self) {
        let instance = self.stack.pop().unwrap();
        self.code.push(Instruction::Data(DataInstruction::Pop));
        self.set_location(instance, None);
    }

    fn push(&mut self, name: Var, value: U256) {
        self.stack.push(VarInstance::Main(name));
        self.code.push(Instruction::Stack(StackInstruction::Push(value.into())));
        self.meta.insert(name, VarMeta {
            main_index: self.stack.len() - 1,
            copy_index: None,
            spill_status: SpillStatus::Unopened,
            spill: Spill::new(&self.code, 0),
        });
    }

    fn stack_swap(&mut self, from_depth: usize, to_depth: usize) -> VarInstance {
        let stack_top = self.stack.len() - 1;
        let from_index = stack_top - from_depth;
        let to_index = stack_top - to_depth;
        let from_instance = self.stack[from_index];
        let to_instance = self.stack[to_index];
        self.stack.swap(from_index, to_index);
        self.set_location(from_instance, Some(to_index));
        self.set_location(to_instance, Some(from_index));
        to_instance
    }

    fn swap_to(&mut self, from_name: Var, to_depth: usize) {
        assert!(to_depth <= 16, "Swap too deep");
        let from_depth = self.find_depth(from_name);
        if from_depth != to_depth {
            self.stack_swap(from_depth, 0);
            self.code.push(Instruction::Stack(StackInstruction::Swap(from_depth)));
            self.set_spill(from_depth, from_name, Spill::new(&self.code, 0));

            let to_instance = self.stack_swap(0, to_depth);
            self.code.push(Instruction::Stack(StackInstruction::Swap(to_depth)));
            self.set_spill(to_depth, to_instance.name(), Spill::new(&self.code, 0))
        }
    }

    fn copy_to(&mut self, from_name: Var, to_depth: usize) {
        assert!(to_depth <= 16, "Copy too deep");
        let from_depth = self.find_depth(from_name);

        let instance = VarInstance::Copy(from_name);
        self.stack.push(instance);
        let index = self.stack.len() - 1;
        self.code.push(Instruction::Stack(StackInstruction::Dup(from_depth)));
        self.set_location(instance, Some(index));
        self.set_spill(from_depth, from_name, Spill::new(&self.code, 0));

        if to_depth != 0 {
            let to_instance = self.stack_swap(0, to_depth);

            self.code.push(Instruction::Stack(StackInstruction::Swap(to_depth)));
            self.set_spill(to_depth, to_instance.name(), Spill::new(&self.code, 0));
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

        self.code.push(Instruction::Data(op));

        for (i, &name) in ress.iter().enumerate() {
            let depth = nress - 1 - i;
            self.meta.insert(name, VarMeta {
                main_index: stack_base + i,
                copy_index: None,
                spill_status: SpillStatus::Unopened,
                spill: Spill::new(&self.code, depth),
            });
        }
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
