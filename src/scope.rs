use eyre::{eyre, Result, Ok};
use std::{borrow::Borrow, collections::HashMap, fmt::Display, hash::Hash};

use crate::program::*;

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
pub struct Var {
    index: u32,
}

impl Var {
    pub fn index(&self) -> usize {
        self.index.try_into().unwrap()
    }
}

struct Env(HashMap<String, Var>);

impl Env {
    fn new() -> Self {
        Env(HashMap::new())
    }

    fn get(&self, name: impl Borrow<String>) -> Result<Var> {
        let key = name.borrow();
        if let Some(&vi) = self.0.get(key) {
            Ok(vi)
        } else {
            Err(eyre!("Unknown variable: {key}"))
        }
    }

    fn insert(&mut self, name: String, value: Var) {
        self.0.insert(name, value);
    }
}

pub struct ResolvedBlock {
    pub block: Block<Var>,
    pub var_count: usize,
}

pub fn resolve(Block(ss): Block<String>) -> Result<ResolvedBlock> {
    let mut env = Env::new();
    let mut i: u32 = 0;

    let ss = ss.into_iter().map(|Statement(vs, e)| {
        let e = match e {
            Expression::Const(c) => Expression::Const(c),

            Expression::Op(op, args) => {
                Expression::Op(
                    op,
                    args.into_iter().map(|x| env.get(x)).collect::<Result<_>>()?,
                )
            }
        };

        let vs = vs.into_iter().map(|v| {
            let vi = Var { index: i };
            i += 1;
            env.insert(v, vi);
            vi
        }).collect();

        Ok(Statement(vs, e))
    }).collect::<Result<_>>()?;

    Ok(ResolvedBlock { block: Block(ss), var_count: i.try_into().unwrap() })
}
