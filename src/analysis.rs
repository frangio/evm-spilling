use std::collections::HashMap;

use crate::{program::{Expression, Statement}, scope::{ResolvedBlock, Var}};

pub fn count_occurrences(rblock: &ResolvedBlock) -> Vec<usize> {
    let mut counts = Vec::new();
    counts.resize(rblock.var_count, 0);
    for Statement(_, e) in &rblock.block.0 {
        match e {
            Expression::Const(_) => (),
            Expression::Op(_, args) => {
                for &x in args {
                    counts[x.index()] += 1;
                }
            }
        }
    }
    counts
}
