use ops;
use flow;
use query;
use backlog;
use ops::NodeOp;
use ops::NodeType;

use std::collections::HashMap;

use shortcut;

/// A union of a set of views.
#[derive(Debug)]
pub struct Union {
    emit: HashMap<flow::NodeIndex, Vec<usize>>,
    srcs: HashMap<flow::NodeIndex, ops::V>,
    cols: HashMap<flow::NodeIndex, usize>,
}

impl Union {
    /// Construct a new union operator.
    ///
    /// When receiving an update from node `a`, a union will emit the columns selected in `emit[a]`.
    pub fn new(emit: HashMap<flow::NodeIndex, Vec<usize>>)
               -> Union {
        Union {
            emit: emit,
            srcs: HashMap::new(),
            cols: HashMap::new(),
        }
    }
}

impl From<Union> for NodeType {
    fn from(b: Union) -> NodeType {
        NodeType::UnionNode(b)
    }
}

impl NodeOp for Union {
    fn prime(&mut self, g: &ops::Graph) -> Vec<flow::NodeIndex> {
        self.srcs.extend(self.emit.keys().map(|&n| (n, g[n].as_ref().unwrap().clone())));
        self.cols.extend(self.srcs.iter().map(|(ni, n)| (*ni, n.args().len())));
        self.emit.keys().cloned().collect()
    }

    fn forward(&self,
               u: ops::Update,
               from: flow::NodeIndex,
               _: i64,
               _: Option<&backlog::BufferedStore>)
               -> Option<ops::Update> {
        match u {
            ops::Update::Records(rs) => {
                Some(ops::Update::Records(rs.into_iter()
                    .map(|rec| {
                        let (r, pos, ts) = rec.extract();

                        // yield selected columns for this source
                        let res = self.emit[&from].iter().map(|&col| r[col].clone()).collect();

                        // return new row with appropriate sign
                        if pos {
                            ops::Record::Positive(res, ts)
                        } else {
                            ops::Record::Negative(res, ts)
                        }
                    })
                    .collect()))
            }
        }
    }

    fn query(&self, q: Option<&query::Query>, ts: i64) -> ops::Datas {
        use std::iter;

        let mut params = HashMap::new();
        for src in self.srcs.keys() {
            params.insert(*src, None);

            // Avoid scanning rows that wouldn't match the query anyway. We do this by finding all
            // conditions that filter over a field present in left, and use those as parameters.
            let emit = &self.emit[src];
            if let Some(q) = q {
                let p: Vec<_> = q.having.iter().map(|c| {
                    shortcut::Condition{
                        column: emit[c.column],
                        cmp: c.cmp.clone(),
                    }
                }).collect();

                if !p.is_empty() {
                    params.insert(*src, Some(p));
                }
            }
        }

        // we select from each source in turn
        params.into_iter()
            .flat_map(move |(src, params)| {
                let emit = &self.emit[&src];
                self.srcs[&src].find(params.map(|cs| {
                    let mut select: Vec<_> = iter::repeat(false).take(self.cols[&src]).collect();
                    for c in emit {
                        select[*c] = true;
                    }
                    query::Query::new(&select[..], cs)
                }), Some(ts)).into_iter()
                // XXX: the clone here is really sad
                .map(move |(r, ts)| (emit.iter().map(|ci| r[*ci].clone()).collect::<Vec<_>>(), ts))
            })
            .filter_map(move |(r, ts)| if let Some(ref q) = q {
                q.feed(&r[..]).map(move |r| (r, ts))
            } else {
                Some((r, ts))
            })
            .collect()
    }

    fn suggest_indexes(&self, _: flow::NodeIndex) -> HashMap<flow::NodeIndex, Vec<usize>> {
        // index nothing (?)
        HashMap::new()
    }

    fn resolve(&self, col: usize) -> Vec<(flow::NodeIndex, usize)> {
        self.emit.iter().map(|(src, emit)| (*src, emit[col])).collect()
    }
}

// yes, this is never satisfied
// tests disabled until we can do dependency injection
#[cfg(all(unix, windows))]
#[cfg(test)]
mod tests {
    use super::*;

    use ops;
    use query;
    use shortcut;

    use ops::NodeOp;
    use std::collections::HashMap;

    fn setup() -> (ops::AQ, Union) {
        // 0 = left, 1 = right
        let mut aqfs = HashMap::new();
        aqfs.insert(0.into(), Box::new(left) as Box<_>);
        aqfs.insert(1.into(), Box::new(right) as Box<_>);

        let mut emits = HashMap::new();
        emits.insert(0.into(), vec![0, 1]);
        emits.insert(1.into(), vec![0, 2]);
        let mut cols = HashMap::new();
        cols.insert(0.into(), 2);
        cols.insert(1.into(), 3);

        let u = Union::new(emits, cols);
        (aqfs, u)
    }

    #[test]
    fn it_works() {
        let (aqfs, u) = setup();

        // forward from left should emit original record
        let left = vec![1.into(), "a".into()];
        match u.forward(left.clone().into(), 0.into(), 0, None, &aqfs).unwrap() {
            ops::Update::Records(rs) => {
                assert_eq!(rs, vec![ops::Record::Positive(left, 0)]);
            }
        }

        // forward from right should emit subset record
        let right = vec![1.into(), "skipped".into(), "x".into()];
        match u.forward(right.clone().into(), 1.into(), 0, None, &aqfs).unwrap() {
            ops::Update::Records(rs) => {
                assert_eq!(rs,
                           vec![ops::Record::Positive(vec![1.into(), "x".into()], 0)]);
            }
        }
    }

    #[test]
    fn it_queries() {
        use std::sync;

        let (aqfs, u) = setup();
        let aqfs = sync::Arc::new(aqfs);

        // do a full query, which should return left + right:
        // [a, b, x]
        let hits = u.query(None, 0, &aqfs);
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().any(|&(ref r, ts)| ts == 0 && r[0] == 1.into() && r[1] == "a".into()));
        assert!(hits.iter().any(|&(ref r, ts)| ts == 1 && r[0] == 2.into() && r[1] == "b".into()));
        assert!(hits.iter().any(|&(ref r, ts)| ts == 2 && r[0] == 1.into() && r[1] == "x".into()));

        // query with parameters matching on both sides
        let q = query::Query::new(&[true, true],
                                  vec![shortcut::Condition {
                             column: 0,
                             cmp: shortcut::Comparison::Equal(shortcut::Value::Const(1.into())),
                         }]);

        let hits = u.query(Some(&q), 0, &aqfs);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|&(ref r, ts)| ts == 0 && r[0] == 1.into() && r[1] == "a".into()));
        assert!(hits.iter().any(|&(ref r, ts)| ts == 2 && r[0] == 1.into() && r[1] == "x".into()));

        // query with parameter matching only on left
        let q = query::Query::new(&[true, true],
                                  vec![shortcut::Condition {
                             column: 0,
                             cmp: shortcut::Comparison::Equal(shortcut::Value::Const(2.into())),
                         }]);

        let hits = u.query(Some(&q), 0, &aqfs);
        assert_eq!(hits.len(), 1);
        assert!(hits.iter().any(|&(ref r, ts)| ts == 1 && r[0] == 2.into() && r[1] == "b".into()));

        // query with parameter matching only on right
        let q = query::Query::new(&[true, true],
                                  vec![shortcut::Condition {
                             column: 1,
                             cmp: shortcut::Comparison::Equal(shortcut::Value::Const("x".into())),
                         }]);

        let hits = u.query(Some(&q), 0, &aqfs);
        assert_eq!(hits.len(), 1);
        assert!(hits.iter().any(|&(ref r, ts)| ts == 2 && r[0] == 1.into() && r[1] == "x".into()));

        // query with parameter with no matches
        let q = query::Query::new(&[true, true],
                                  vec![shortcut::Condition {
                             column: 0,
                             cmp: shortcut::Comparison::Equal(shortcut::Value::Const(3.into())),
                         }]);

        let hits = u.query(Some(&q), 0, &aqfs);
        assert_eq!(hits.len(), 0);
    }

    fn left(p: ops::Params, _: i64) -> Vec<(Vec<query::DataType>, i64)> {
        let data = vec![
                (vec![1.into(), "a".into()], 0),
                (vec![2.into(), "b".into()], 1),
            ];

        assert_eq!(p.len(), 2);
        let mut p = p.into_iter();
        let q = query::Query::new(&[true, true],
                                  vec![
                shortcut::Condition {
                             column: 0,
                             cmp: shortcut::Comparison::Equal(p.next().unwrap()),
                         },
                shortcut::Condition {
                             column: 1,
                             cmp: shortcut::Comparison::Equal(p.next().unwrap()),
                         },
            ]);

        data.into_iter().filter_map(move |(r, ts)| q.feed(&r[..]).map(|r| (r, ts))).collect()
    }

    fn right(p: ops::Params, _: i64) -> Vec<(Vec<query::DataType>, i64)> {
        let data = vec![
                (vec![1.into(), "skipped".into(), "x".into()], 2),
            ];

        assert_eq!(p.len(), 3);
        let mut p = p.into_iter();
        let q = query::Query::new(&[true, true, true],
                                  vec![shortcut::Condition {
                                           column: 0,
                                           cmp: shortcut::Comparison::Equal(p.next().unwrap()),
                                       },
                                       shortcut::Condition {
                                           column: 1,
                                           cmp: shortcut::Comparison::Equal(p.next().unwrap()),
                                       },
                                       shortcut::Condition {
                                           column: 2,
                                           cmp: shortcut::Comparison::Equal(p.next().unwrap()),
                                       }]);

        data.into_iter().filter_map(move |(r, ts)| q.feed(&r[..]).map(|r| (r, ts))).collect()
    }

    #[test]
    fn it_suggests_indices() {
        let (_, u) = setup();
        assert_eq!(HashMap::new(), u.suggest_indexes(1.into()));
    }

    #[test]
    fn it_resolves() {
        let (_, u) = setup();
        let r0 = u.resolve(0);
        assert!(r0.iter().any(|&(n, c)| n == 0.into() && c == 0));
        assert!(r0.iter().any(|&(n, c)| n == 1.into() && c == 0));
        let r1 = u.resolve(1);
        assert!(r1.iter().any(|&(n, c)| n == 0.into() && c == 1));
        assert!(r1.iter().any(|&(n, c)| n == 1.into() && c == 2));
    }
}
