use crdts::lseq::ident::*;
use crdts::lseq::*;
use crdts::CmRDT;
use rand::distributions::Alphanumeric;
use rand::Rng;

#[test]
fn test_out_of_order_inserts() {
    let mut site1 = LSeq::new(SiteId::new(0));
    site1.insert_index(0, 'a');
    site1.insert_index(1, 'c');
    site1.insert_index(1, 'b');

    assert_eq!(site1.iter().collect::<String>(), "abc");
}

#[test]
fn test_inserts() {
    // A simple smoke test to ensure that insertions work properly.
    // Uses two sites which random insert a character and then immediately insert it into the
    // other site.
    let mut rng = rand::thread_rng();

    let mut s1 = rng.sample_iter(Alphanumeric);
    let mut s2 = rng.sample_iter(Alphanumeric);

    let mut site1 = LSeq::new(SiteId::new(0));
    let mut site2 = LSeq::new(SiteId::new(1));

    for _ in 0..5000 {
        if rng.gen() {
            let op = site1.insert_index(
                rng.gen_range(0, site1.raw_entries().len() + 1),
                s1.next().unwrap(),
            );
            site2.apply(op);
        } else {
            let op = site2.insert_index(
                rng.gen_range(0, site2.raw_entries().len() + 1),
                s2.next().unwrap(),
            );
            site1.apply(op);
        }
    }
    assert_eq!(
        site1.iter().collect::<String>(),
        site2.iter().collect::<String>()
    );
}

#[derive(Clone)]
struct OperationList(pub Vec<Op<char>>);

use quickcheck::{Arbitrary, Gen};

impl Arbitrary for OperationList {
    fn arbitrary<G: Gen>(g: &mut G) -> OperationList {
        let size = {
            let s = g.size();
            if s == 0 {
                0
            } else {
                g.gen_range(0, s)
            }
        };

        let mut site1 = LSeq::new(SiteId::new(g.gen()));
        let ops = (0..size)
            .filter_map(|_| {
                if g.gen() || site1.len() == 0 {
                    site1.delete_index(g.gen_range(0, site1.len() + 1))
                } else {
                    site1.delete_index(g.gen_range(0, site1.len()))
                }
            })
            .collect();
        OperationList(ops)
    }
    // implement shrinking ://
}

#[test]
fn prop_inserts_and_deletes() {
    let mut rng = quickcheck::StdThreadGen::new(1000);
    let mut op1 = OperationList::arbitrary(&mut rng).0.into_iter();
    let mut op2 = OperationList::arbitrary(&mut rng).0.into_iter();

    let mut site1 = LSeq::new(SiteId::new(0));
    let mut site2 = LSeq::new(SiteId::new(1));

    let mut s1_empty = false;
    let mut s2_empty = false;
    while !s1_empty && !s2_empty {
        if rng.gen() {
            match op1.next() {
                Some(o) => {
                    site1.apply(o.clone());
                    site2.apply(o);
                }
                None => {
                    s1_empty = true;
                }
            }
        } else {
            match op2.next() {
                Some(o) => {
                    site1.apply(o.clone());
                    site2.apply(o);
                }
                None => {
                    s2_empty = true;
                }
            }
        }
    }

    let site1_text = site1.iter().collect::<String>();
    let site2_text = site2.iter().collect::<String>();

    assert_eq!(site1_text, site2_text);
}
