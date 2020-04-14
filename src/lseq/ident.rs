use crate::vclock::Actor;
use bitvec::vec::*;
use rand::Rng;
use serde::{Deserialize, Serialize};

const BOUNDARY: u64 = 10;

const INITIAL_BASE: u32 = 3; // start with 2^8

/// A tree identifier uniquely locates a character in an LSeq tree.
/// It represents the path that needs to be taken in order to reach
/// the character. At each level we store the index of the child tree node
/// as well as the id of the site that inserted that node. This resolves conflicts where
/// two sites decide to pick the same child index to allocate a fresh node
#[derive(Debug, PartialEq, PartialOrd, Ord, Eq, Clone, Serialize, Deserialize, Hash)]
pub struct Identifier<A: Actor> {
    path: Vec<(u64, Option<A>)>,
}

impl<A: Actor> Identifier<A> {
    /// Get the size of an identifier.
    pub fn len(&self) -> usize {
        self.path.len()
    }

    /// Check if the identifier is empty.
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }
}

/// A generator for fresh identifiers.
///
/// These identifiers represent a path in an exponential tree. At each level of the tree the amount
/// of children doubles. This means that we can store a lot of nodes in a very low height!
///
/// The [`crate::LSeq`] tree also has an additional restriction: at each level the minimum and maximum nodes
/// cannot be chosen when allocating fresh nodes. This is to ensure there is always a free node
/// that can be used to create a lower level.
//#[derive(Serialize, Deserialize)]
#[derive(Debug, Clone)]
pub struct IdentGen<A: Actor> {
    initial_base_bits: u32,
    strategy_vec: BitVec,
    /// Site id of this tree
    pub site_id: A,
}

impl<A: Actor> IdentGen<A> {
    /// Create a fresh tree with 0 node.
    pub fn new(site_id: A) -> Self {
        Self::new_with_args(INITIAL_BASE, site_id)
    }

    /// Create a tree with a custom initial size.
    pub fn new_with_args(base: u32, site_id: A) -> Self {
        IdentGen {
            initial_base_bits: base,
            strategy_vec: BitVec::new(),
            site_id,
        }
    }

    /// The smallest possible node in a tree.
    pub fn lower(&self) -> Identifier<A> {
        Identifier {
            path: vec![(0, None)],
        }
    }

    /// The absolute largest possible node.
    pub fn upper(&self) -> Identifier<A> {
        Identifier {
            path: vec![(2u64.pow(self.initial_base_bits) - 1, None)],
        }
    }

    /// Allocates a new identifier between `p` and `q` such that `p < z < q`.
    ///
    /// The way to think about this is to consider the problem of finding a short number between
    /// two decimal numbers. For example, between `0.2` and `0.4` we'd like to return `0.3`.
    /// But what about `0.2` and `0.3`? Well a nice property is that if we add a digit to `0.2`, the
    /// number produced will be larger than `0.2` but smaller than `0.3`.
    ///
    /// If you view identifiers `p` and `q` as decimal numbers `0.p` and `0.q` then all we're doing is
    /// finding a number between them!
    ///
    /// # Panics
    ///
    /// * `p` equal to `q`.
    /// * If no identifier of length less than 31 exists between `p` and `q`.
    pub fn alloc(&mut self, p: &Identifier<A>, q: &Identifier<A>) -> Identifier<A> {
        assert!(p != q, "allocation range should be non-empty");
        if q < p {
            self.alloc(q, p);
        }

        let mut depth = 0;

        // Descend both identifiers in parallel until we find room to insert an identifier
        loop {
            match (p.path.get(depth), q.path.get(depth)) {
                // Our descent continues...
                (Some(a), Some(b)) => {
                    if a.0 == b.0 {
                        // continue searching lower
                    } else if a.0 + 1 < b.0 {
                        // We take this branch if there's room between a and b to insert a new
                        // identifier
                        let next_index = self.index_in_range(a.0 + 1, b.0, depth as u32);
                        return self.replace_last(p, depth, next_index);
                    } else {
                        // If we end up here that means that a + 1 = b. Therefore we can't actually
                        // insert a new node at this layer. So we start a new layer below a.
                        return self.alloc_from_lower(p, depth + 1);
                    }
                }

                // The upper bound ended on the previous level.
                // This means we can allocate a node in the range a..max_for_depth
                // If there isn't room at this level because a = max_for_depth then we'll keep
                // searching below.
                (Some(_), None) => {
                    return self.alloc_from_lower(p, depth);
                }
                // The lower bound ended on the previous level, we just need to keep following the upper bound until some space
                // is available.
                (None, Some(_)) => {
                    return self.alloc_from_upper(p, q, depth);
                }

                // The two paths are fully equal which means that the site_ids MUST be different or
                // we are in an invalid situation
                (None, None) => {
                    let max_for_depth = self.width_at(depth as u32) - 1;
                    let next_index = self.index_in_range(1, max_for_depth, depth as u32);
                    return self.push_index(p, next_index);
                }
            };
            depth += 1;
        }
    }

    // Here we have the lowest possible upper bound and we just need to traverse the lower bound
    // until we can find somehwere to insert a new identifier.
    //
    // This reflects the case where we want to allocate between 0.20001 and 0.3
    fn alloc_from_lower(&mut self, p: &Identifier<A>, mut depth: usize) -> Identifier<A> {
        loop {
            match p.path.get(depth) {
                Some((ix, _)) => {
                    if ix + 1 < self.width_at(depth as u32) {
                        let next_index =
                            self.index_in_range(ix + 1, self.width_at(depth as u32), depth as u32);
                        return self.replace_last(p, depth, next_index);
                    }
                }
                None => {
                    let next_index =
                        self.index_in_range(1, self.width_at(depth as u32), depth as u32);
                    return self.push_index(p, next_index);
                }
            }
            depth += 1;
        }
    }

    // Here the problem is that we've run out of the lower path and the upper one is zero!
    // The idea is to keep pushing 0s onto the lower path until we can find a new level to allocate at.
    //
    // This reflects the case where we want to allocate something between 0.2 and 0.200000001
    fn alloc_from_upper(
        &mut self,
        base: &Identifier<A>,
        q: &Identifier<A>,
        mut depth: usize,
    ) -> Identifier<A> {
        let mut ident = base.clone();
        loop {
            match q.path.get(depth) {
                // append a 0 to the result path as well
                Some(b) if b.0 <= 1 => ident.path.push((0, b.1.clone())),
                // oo! a non-zero value
                _ => break,
            }
            depth += 1;
        }

        // If we actually ran out of upper bound values then we're free to choose
        // anything on the next level, otherwise use the upper bound we've found.
        let upper = match q.path.get(depth) {
            Some(b) => b.0,
            None => self.width_at(depth as u32 + 1),
        };
        let next_index = self.index_in_range(1, upper, depth as u32);
        self.push_index(&ident, next_index)
    }

    fn replace_last(&mut self, p: &Identifier<A>, depth: usize, ix: u64) -> Identifier<A> {
        let mut ident = p.clone();
        ident.path.truncate(depth);
        ident.path.push((ix, Some(self.site_id.clone())));
        ident
    }

    fn push_index(&mut self, p: &Identifier<A>, ix: u64) -> Identifier<A> {
        let mut ident = p.clone();
        ident.path.push((ix, Some(self.site_id.clone())));
        ident
    }

    fn width_at(&self, depth: u32) -> u64 {
        assert!(
            self.initial_base_bits + depth < 31,
            "maximum depth exceeded"
        );

        2u64.pow(self.initial_base_bits + depth)
    }
    // Generate an index in a given range at the specified depth.
    // Uses the allocation strategy of that depth, boundary+ or boundary- which is biased to the
    // lower and upper ends of the range respectively.
    // should allocate in the range [lower, upper)
    fn index_in_range(&mut self, lower: u64, upper: u64, depth: u32) -> u64 {
        assert!(
            lower < upper,
            "need at least one space between the bounds lower={} upper={}",
            lower,
            upper
        );

        let mut rng = rand::rngs::OsRng;
        let interval = BOUNDARY.min(upper - 1 - lower);
        let step = if interval > 0 {
            rng.gen_range(0, interval)
        } else {
            0
        };
        if self.strategy(depth) {
            //boundary+
            lower + step
        } else {
            //boundary-
            upper - step - 1
        }
    }

    fn strategy(&mut self, depth: u32) -> bool {
        match self.strategy_vec.get(depth as usize) {
            None => {
                self.strategy_vec.push(rand::thread_rng().gen());
                self.strategy(depth)
            }
            Some(s) => *s,
        }
        // temp strategy. Should be a random choice at each level
    }
}

#[cfg(test)]
impl<A: quickcheck::Arbitrary + Actor> quickcheck::Arbitrary for Identifier<A> {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Identifier<A> {
        Identifier {
            path: Vec::<(u64, Option<A>)>::arbitrary(g),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use quickcheck::{quickcheck, Arbitrary, Gen, TestResult};

    fn valid_identifier(id: &Identifier<u32>) -> bool {
        let mut d = 0;
        if id.path.len() == 0 || id.len() > 27 {
            return false;
        }
        for (p, _) in id.path.iter() {
            if p > &2u64.pow(3 + d) {
                return false;
            }
            d += 1;
        }
        true
    }

    quickcheck! {
        fn prop_alloc(p: Identifier<u32>, q: Identifier<u32>) -> TestResult {
            if ! valid_identifier(&p) || ! valid_identifier(&q) {
                return TestResult::discard();
            }
            let mut gen = IdentGen::new(0);
            let z = gen.alloc(&p, &q);

            TestResult::from_bool( (p < z && z < q) || (q < z && z < p))
        }
    }

    #[test]
    fn test_alloc_eq_path() {
        let mut gen = IdentGen::new(0);

        let x = Identifier {
            path: vec![(1, Some(0)), (1, Some(0))],
        };
        let y = Identifier {
            path: vec![(1, Some(0)), (1, Some(1))],
        };
        gen.alloc(&x, &y);
        let b = gen.alloc(&x, &y);
        // println!("{:?} {:?} {:?}", x, b, y);
        assert!(x < b);
        assert!(b < y);
    }

    #[test]
    fn test_different_len_paths() {
        let mut gen = IdentGen::new(0);
        let x = Identifier {
            path: vec![(1, Some(0))],
        };
        let y = Identifier {
            path: vec![(1, Some(0)), (15, Some(0))],
        };

        let z = gen.alloc(&x, &y);

        assert!(x < z);
        assert!(z < y);
    }

    #[test]
    fn test_alloc() {
        let mut gen = IdentGen::new(0);
        let a = Identifier {
            path: vec![(1, Some(0))],
        };
        let b = Identifier {
            path: vec![(3, Some(0))],
        };

        assert_eq!(
            gen.alloc(&a, &b),
            Identifier {
                path: vec![(2, Some(0))]
            }
        );

        let c = Identifier {
            path: vec![(1, Some(0)), (0, Some(0)), (1, Some(0))],
        };
        let d = Identifier {
            path: vec![(1, Some(0)), (0, Some(0)), (3, Some(0))],
        };

        assert_eq!(
            gen.alloc(&c, &d),
            Identifier {
                path: vec![(1, Some(0)), (0, Some(0)), (2, Some(0))]
            }
        );

        let e = Identifier {
            path: vec![(1, Some(0))],
        };
        let f = Identifier {
            path: vec![(2, Some(0))],
        };

        let res = gen.alloc(&e, &f);

        assert!(e < res);
        assert!(res < f);
        {
            let mut gen = IdentGen::new(1);

            let a = Identifier {
                path: vec![(4, Some(0)), (4, Some(0))],
            };
            let b = Identifier {
                path: vec![(4, Some(0)), (4, Some(0)), (1, Some(1))],
            };

            let c = gen.alloc(&a, &b);
            assert!(a < c);
            assert!(c < b);
        }
        {
            let a = Identifier {
                path: vec![(5, Some(1)), (6, Some(1)), (6, Some(1)), (6, Some(0))],
            };
            let b = Identifier {
                path: vec![
                    (5, Some(1)),
                    (6, Some(1)),
                    (6, Some(1)),
                    (6, Some(0)),
                    (0, Some(0)),
                    (507, Some(0)),
                ],
            };

            let c = gen.alloc(&a, &b);
            assert!(a < c);
            assert!(c < b);
        }
    }

    #[test]
    fn test_index_in_range() {
        let mut gen = IdentGen::new(0);
        assert_eq!(gen.index_in_range(0, 1, 1), 0);
    }
}
