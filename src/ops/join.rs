use ops;
use query;
use shortcut;

use std::iter;
use std::collections::HashMap;

use flow::prelude::*;

#[derive(Debug)]
struct JoinTarget {
    fields: Vec<(usize, usize)>,
    select: Vec<bool>,
    outer: bool,
}

#[derive(Debug)]
struct Join {
    against: HashMap<NodeIndex, JoinTarget>,
    node: NodeIndex,
}

/// Convenience struct for building join nodes.
pub struct Builder {
    emit: Vec<(NodeIndex, usize)>,
    join: HashMap<NodeIndex, (bool, Vec<usize>)>,
}

impl Builder {
    /// Build a new join operator.
    ///
    /// `emit` dictates, for each output column, which source and column should be used.
    pub fn new(emit: Vec<(NodeIndex, usize)>) -> Self {
        Builder {
            emit: emit,
            join: HashMap::new(),
        }
    }

    /// Set the source view for this join.
    ///
    /// This is semantically identical to `join`, except that it also asserts that this is the
    /// first view being added. The first view is of particular importance as it dictates the
    /// behavior of later *left* joins (when they are added).
    pub fn from(self, node: NodeIndex, groups: Vec<usize>) -> Self {
        assert!(self.join.is_empty());
        self.join(node, groups)
    }

    /// Also join with the given `node`.
    ///
    /// `groups` gives the group assignments for each output column of `node`. Columns across join
    /// sources that share a group are used to join rows from those sources by equality. Thus, each
    /// group number can appear at most once for each view.
    ///
    /// Let us look at a SQL join such as
    ///
    /// ```sql
    /// SELECT a.0, b.0
    /// FROM a JOIN b USING (a.0 == b.1)
    /// ```
    ///
    /// Assuming `a` has two columns and `b` has three, the map would look like this:
    ///
    /// ```rust,ignore
    /// Builder::new(vec![(a, 0), (b, 0)]).from(a, vec![1, 0]).join(b, vec![0, 1, 0]);
    /// ```
    pub fn join(mut self, node: NodeIndex, groups: Vec<usize>) -> Self {
        assert!(self.join.insert(node, (false, groups)).is_none());
        self
    }

    /// Also perform a left join against the given `node`.
    ///
    /// The semantics of this is similar to the SQL notion of a `LEFT JOIN`, namely that records
    /// from other tables that join against this table will always be present in the output,
    /// regardless of whether matching records exist in `node`. For such *zero rows*, all columns
    /// emitted from this node will be set to `DataType::None`.
    pub fn left_join(mut self, node: NodeIndex, groups: Vec<usize>) -> Self {
        assert!(self.join.insert(node, (true, groups)).is_none());
        self
    }
}

impl From<Builder> for Joiner {
    fn from(b: Builder) -> Joiner {
        if b.join.len() != 2 {
            // only two-way joins are currently supported
            unimplemented!();
        }

        // we technically want this assert, but we don't have self.nodes until .prime() has been
        // called. unfortunately, at that time, we don't have .join in the original format, and so
        // the debug doesn't makes sense. it's probably not worth carrying along the original join
        // map just to verify this, but maybe...
        // assert!(self.nodes.iter().all(|(ni, n)| self.join[ni].len() == n.args().len()));

        // the format of `join` is convenient for users, but not particulary convenient for lookups
        // the particular use-case we want to be efficient is:
        //
        //  - we are given a record from `src`
        //  - for each other parent `p`, we want to know which columns of `p` to constrain, and
        //    which values in the `src` record those correspond to
        //
        // so, we construct a map of the form
        //
        //   src: NodeIndex => {
        //     p: NodeIndex => [(srci, pi), ...]
        //   }
        //
        let join = b.join
            .iter()
            .map(|(&src, &(_, ref srcg))| {
                // which groups are bound to which columns?
                let g2c = srcg.iter()
                    .enumerate()
                    .filter_map(|(c, &g)| if g == 0 { None } else { Some((g, c)) })
                    .collect::<HashMap<_, _>>();

                // for every other view
                let other = b.join
                    .iter()
                    .filter_map(|(&p, &(outer, ref pg))| {
                        // *other* view
                        if p == src {
                            return None;
                        }
                        // look through the group assignments for that other view
                        let pg: Vec<_> = pg.iter()
                            .enumerate()
                            .filter_map(|(pi, g)| {
                                // look for ones that share a group with us
                                g2c.get(g).map(|srci| {
                                    // and emit that mapping
                                    (*srci, pi)
                                })
                            })
                            .collect();

                        // if there are no shared columns, don't join against this view
                        if pg.is_empty() {
                            return None;
                        }
                        // but if there are, emit the mapping we found
                        Some((p,
                              JoinTarget {
                                  fields: pg,
                                  outer: outer,
                                  select: Vec::new(),
                              }))
                    })
                    .collect();

                (src,
                 Join {
                     against: other,
                     node: src,
                 })
            })
            .collect();

        Joiner {
            emit: b.emit,
            join: join,
        }
    }
}

use flow::node;
impl Into<node::Type> for Builder {
    fn into(self) -> node::Type {
        let j: Joiner = self.into();
        node::Type::Unassigned(Box::new(j) as Box<Ingredient>)
    }
}

/// Joiner provides a 2-way join between two views.
///
/// It shouldn't be *too* hard to extend this to `n`-way joins, but it would require restructuring
/// `.join` such that it can express "query this view first, then use one of its columns to query
/// this other view".
#[derive(Debug)]
pub struct Joiner {
    emit: Vec<(NodeIndex, usize)>,
    join: HashMap<NodeIndex, Join>,
}

impl Joiner {
    fn join<'a>(&'a self,
                left: (NodeIndex, Vec<query::DataType>),
                domain: &DomainNodes,
                states: &StateMap)
                -> Box<Iterator<Item = Vec<query::DataType>> + 'a> {

        // NOTE: this only works for two-way joins
        let other = *self.join.keys().find(|&other| other != &left.0).unwrap();
        let this = &self.join[&left.0];
        let target = &this.against[&other];

        let rx = {
            // figure out the join values for this record
            let params = target.fields
                .iter()
                .map(|&(lefti, righti)| {
                    shortcut::Condition {
                        column: righti,
                        cmp: shortcut::Comparison::Equal(shortcut::Value::using(&left.1[lefti])),
                    }
                })
                .collect();

            // TODO: technically, we only need the columns in .join and .emit
            let q = query::Query::new(&target.select[..], params);

            // send the parameters to start the query.
            // TODO: avoid duplicating this exact code in every querying module
            if let Some(state) = states.get(&other) {
                // other node is materialized
                state.find(&q.having[..]).map(|r| r.iter().cloned().collect()).collect()
            } else {
                // other node is not materialized, query instead
                domain[&other].borrow().query(Some(&q), domain, states)
            }
        };

        if rx.is_empty() && target.outer {
            return Box::new(Some(self.emit
                    .iter()
                    .map(|&(source, column)| {
                        if source == other {
                            query::DataType::None
                        } else {
                            // this clone is unnecessary
                            left.1[column].clone()
                        }
                    })
                    .collect::<Vec<_>>())
                .into_iter());
        }

        Box::new(rx.into_iter().map(move |right| {
            // weave together r and j according to join rules
            self.emit
                .iter()
                .map(|&(source, column)| {
                    if source == other {
                        // FIXME: this clone is unnecessary.
                        // it's tricky to remove though, because it means we'd need to
                        // be removing things from right. what if a later column also needs
                        // to select from right? we'd need to keep track of which things we
                        // have removed, and subtract that many from the index of the
                        // later column. ugh.
                        right[column].clone()
                    } else {
                        left.1[column].clone()
                    }
                })
                .collect()
        }))
    }
}

impl Ingredient for Joiner {
    fn ancestors(&self) -> Vec<NodeIndex> {
        self.join.keys().cloned().collect()
    }

    fn should_materialize(&self) -> bool {
        false
    }

    fn will_query(&self, _: bool) -> bool {
        true
    }

    fn on_connected(&mut self, g: &Graph) {
        for (_, j) in &mut self.join {
            for (t, jt) in &mut j.against {
                jt.select = iter::repeat(true)
                    .take(g[*t].fields().len())
                    .collect::<Vec<_>>();
            }
        }
    }

    fn on_commit(&mut self, _: NodeIndex, remap: &HashMap<NodeIndex, NodeIndex>) {
        // our ancestors may have been remapped
        // we thus need to fix up any node indices that could have changed
        for (from, to) in remap {
            if from == to {
                continue;
            }

            if let Some(mut j) = self.join.remove(from) {
                j.node = *to;
                assert!(self.join.insert(*to, j).is_none());
            }

            for j in self.join.values_mut() {
                if let Some(t) = j.against.remove(from) {
                    assert!(j.against.insert(*to, t).is_none());
                }
            }
        }

        for &mut (ref mut ni, _) in &mut self.emit {
            *ni = remap[&*ni];
        }
    }

    fn on_input(&mut self,
                input: Message,
                nodes: &DomainNodes,
                state: &StateMap)
                -> Option<Update> {
        let from = input.from;
        match input.data {
            ops::Update::Records(rs) => {
                // okay, so here's what's going on:
                // the record(s) we receive are all from one side of the join. we need to query the
                // other side(s) for records matching the incoming records on that side's join
                // fields.

                // TODO: we should be clever here, and only query once per *distinct join value*,
                // instead of once per received record.
                ops::Update::Records(rs.into_iter()
                        .flat_map(|rec| {
                            let (r, pos) = rec.extract();

                            self.join((from, r), nodes, state).map(move |res| {
                                // return new row with appropriate sign
                                if pos {
                                    ops::Record::Positive(res)
                                } else {
                                    ops::Record::Negative(res)
                                }
                            })
                        })
                        .collect())
                    .into()
            }
        }
    }

    fn query(&self,
             q: Option<&query::Query>,
             domain: &DomainNodes,
             states: &StateMap)
             -> ops::Datas {
        use std::iter;

        // We're essentially doing nested for loops, where each loop yields rows from one "table".
        // For the case of a two-way join (which is all that's supported for now), we call the two
        // tables `left` and `right`. We're going to iterate over results from `left` in the outer
        // loop, and query `right` inside the loop for each `left`.

        // pick some view query order
        // TODO: figure out which join order is best
        let lefti = *self.join.keys().min().unwrap();
        let left = &self.join[&lefti];

        // Set up parameters for querying all rows in left.
        //
        // We find the number of parameters by looking at how many parameters the other side of the
        // join would have used if it tried to query us.
        let mut lparams = None;

        // Avoid scanning rows that wouldn't match the query anyway. We do this by finding all
        // conditions that filter over a field present in left, and use those as parameters.
        if let Some(q) = q {
            lparams = Some(q.having
                .iter()
                .filter_map(|c| {
                    let (srci, coli) = self.emit[c.column];
                    if srci != lefti {
                        return None;
                    }

                    Some(shortcut::Condition {
                        column: coli,
                        cmp: c.cmp.clone(),
                    })
                })
                .collect::<Vec<_>>());

            if lparams.as_ref().unwrap().is_empty() {
                lparams = None;
            }
        }

        // produce a left * right given a left (basically the same as forward())
        // TODO: we probably don't need to select all columns here
        let lq = lparams.map(|ps| {
            query::Query::new(&iter::repeat(true)
                                  .take(domain[&left.node].borrow().fields().len())
                                  .collect::<Vec<_>>(),
                              ps)
        });

        let leftrx = if let Some(state) = states.get(&left.node) {
            // other node is materialized
            state.find(lq.as_ref().map(|q| &q.having[..]).unwrap_or(&[]))
                .map(|r| r.iter().cloned().collect())
                .collect()
        } else {
            // other node is not materialized, query instead
            domain[&left.node].borrow().query(lq.as_ref(), domain, states)
        };

        leftrx.into_iter()
            .flat_map(move |lrec| {
                // TODO: also add constants from q to filter used to select from right
                // TODO: respect q.select
                self.join((lefti, lrec), domain, states)
            })
            .filter_map(move |r| if let Some(q) = q {
                q.feed(r).map(|r| r)
            } else {
                Some(r)
            })
            .collect()
    }

    fn suggest_indexes(&self, this: NodeIndex) -> HashMap<NodeIndex, Vec<usize>> {
        use std::collections::HashSet;

        // index all join fields
        self.join
            .iter()
            // for every left
            .flat_map(|(left, rs)| {
                // for every right
                rs.against.iter().flat_map(move |(right, rs)| {
                    // emit both the left binding
                    rs.fields.iter().map(move |&(li, _)| (left, li))
                    // and the right binding
                    .chain(rs.fields.iter().map(move |&(_, ri)| (right, ri)))
                })
            })
            // we now have (NodeIndex, usize) for every join column.
            .fold(HashMap::new(), |mut hm, (node, col)| {
                hm.entry(*node).or_insert_with(HashSet::new).insert(col);

                // if this join column is emitted, we also want an index on that output column, as
                // it's likely the user will do lookups on it.
                if let Some(outi) = self.emit.iter().position(|&(ref n, c)| n == node && c == col) {
                    hm.entry(this).or_insert_with(HashSet::new).insert(outi);
                }
                hm
            })
            // convert HashSets into Vec
            .into_iter().map(|(node, cols)| (node, cols.into_iter().collect())).collect()
    }

    fn resolve(&self, col: usize) -> Option<Vec<(NodeIndex, usize)>> {
        Some(vec![self.emit[col].clone()])
    }

    fn description(&self) -> String {
        let emit = self.emit
            .iter()
            .map(|&(src, col)| format!("{}:{}", src.index(), col))
            .collect::<Vec<_>>()
            .join(", ");
        let joins = self.join
            .iter()
            .flat_map(|(left, rs)| {
                rs.against
                    .iter()
                    .filter(move |&(right, _)| left < right)
                    .flat_map(move |(right, rs)| {
                        let op = if rs.outer { "⋉" } else { "⋈" };
                        rs.fields.iter().map(move |&(li, ri)| {
                            format!("{}:{} {} {}:{}", left.index(), li, op, right.index(), ri)
                        })
                    })
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("[{}] {}", emit, joins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ops;
    use query;
    use shortcut;

    fn setup(left: bool) -> (ops::test::MockGraph, NodeIndex, NodeIndex) {
        let mut g = ops::test::MockGraph::new();
        let l = g.add_base("left", &["l0", "l1"]);
        let r = g.add_base("right", &["r0", "r1"]);

        g.seed(l, vec![1.into(), "a".into()]);
        g.seed(l, vec![2.into(), "b".into()]);
        g.seed(l, vec![3.into(), "c".into()]);
        g.seed(r, vec![1.into(), "x".into()]);
        g.seed(r, vec![1.into(), "y".into()]);
        g.seed(r, vec![2.into(), "z".into()]);

        // join on first field
        let b = Builder::new(vec![(l, 0), (l, 1), (r, 1)]).from(l, vec![1, 0]);
        let b = if left {
            b.left_join(r, vec![1, 0])
        } else {
            b.join(r, vec![1, 0])
        };

        let j: Joiner = b.into();
        g.set_op("join", &["j0", "j1", "j2"], j);
        (g, l, r)
    }

    #[test]
    fn it_describes() {
        let (j, l, r) = setup(false);
        let l = l.index();
        let r = r.index();
        assert_eq!(j.node().description(),
                   format!("[{}:0, {}:1, {}:1] {}:0 ⋈ {}:0", l, l, r, l, r));
    }

    #[test]
    fn it_describes_left() {
        let (j, l, r) = setup(true);
        let l = l.index();
        let r = r.index();
        assert_eq!(j.node().description(),
                   format!("[{}:0, {}:1, {}:1] {}:0 ⋉ {}:0", l, l, r, l, r));
    }

    fn forward_non_weird(mut j: ops::test::MockGraph, l: NodeIndex, r: NodeIndex) {
        // these are the data items we have to work with
        // these are in left
        let l_a1 = vec![1.into(), "a".into()];
        let l_b2 = vec![2.into(), "b".into()];
        // let l_c3 = vec![3.into(), "c".into()]; // considered weird
        // these are in right
        let r_x1 = vec![1.into(), "x".into()];
        let r_y1 = vec![1.into(), "y".into()];
        let r_z2 = vec![2.into(), "z".into()];

        // *************************************
        // forward from the left
        // *************************************

        // forward b2 from left; should produce [b2*z2]
        match j.one_row(l, l_b2.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // we're expecting to only match z2
                assert_eq!(rs,
                           vec![ops::Record::Positive(vec![2.into(), "b".into(), "z".into()])]);
            }
        }

        // forward a1 from left; should produce [a1*x1, a1*y1]
        match j.one_row(l, l_a1.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // we're expecting two results: x1 and y1
                assert_eq!(rs.len(), 2);
                // they should all be positive since input was positive
                assert!(rs.iter().all(|r| r.is_positive()));
                // they should all have the correct values from the provided left
                assert!(rs.iter().all(|r| r.rec()[0] == 1.into() && r.rec()[1] == "a".into()));
                // and both join results should be present
                assert!(rs.iter().any(|r| r.rec()[2] == "x".into()));
                assert!(rs.iter().any(|r| r.rec()[2] == "y".into()));
            }
        }

        // *************************************
        // forward from the right
        // *************************************

        // forward x1 from right; should produce [a1*x1]
        match j.one_row(r, r_x1.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                assert_eq!(rs,
                           vec![ops::Record::Positive(vec![1.into(), "a".into(), "x".into()])]);
            }
        }

        // forward y1 from right; should produce [a1*y1]
        match j.one_row(r, r_y1.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // NOTE: because we use r_y1.into(), left's timestamp will be set to 0
                assert_eq!(rs,
                           vec![ops::Record::Positive(vec![1.into(), "a".into(), "y".into()])]);
            }
        }

        // forward z2 from right; should produce [b2*z2]
        match j.one_row(r, r_z2.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // NOTE: because we use r_z2.into(), left's timestamp will be set to 0, and thus
                // right's (b2's) timestamp will be used.
                assert_eq!(rs,
                           vec![ops::Record::Positive(vec![2.into(), "b".into(), "z".into()])]);
            }
        }
    }

    #[test]
    fn it_works() {
        let (mut j, l, r) = setup(false);
        let l_c3 = vec![3.into(), "c".into()];

        // forward c3 from left; should produce [] since no records in right are 3
        match j.one_row(l, l_c3.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // right has no records with value 3
                assert_eq!(rs.len(), 0);
            }
        }

        forward_non_weird(j, l, r);
    }

    #[test]
    fn it_works_left() {
        let (mut j, l, r) = setup(true);

        let l_c3 = vec![3.into(), "c".into()];

        // forward c3 from left; should produce [c3 + None] since no records in right are 3
        match j.one_row(l, l_c3.clone(), false).unwrap() {
            ops::Update::Records(rs) => {
                // right has no records with value 3, so we're expecting a single record with None
                // for all columns output from the (non-existing) right record
                assert_eq!(rs.len(), 1);
                // that row should be positive
                assert!(rs.iter().all(|r| r.is_positive()));
                // and should have the correct values from the provided left
                assert!(rs.iter().all(|r| r.rec()[0] == 3.into() && r.rec()[1] == "c".into()));
                // and None for the remaining column
                assert!(rs.iter().any(|r| r.rec()[2] == query::DataType::None));
            }
        }

        forward_non_weird(j, l, r);
    }

    #[test]
    fn it_queries() {
        let (j, _, _) = setup(false);

        // do a full query, which should return product of left + right:
        // [ax, ay, bz]
        let hits = j.query(None);
        assert_eq!(hits.len(), 3);
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "x".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "y".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 2.into() && r[1] == "b".into() && r[2] == "z".into()));

        // query using join field
        let val = shortcut::Comparison::Equal(shortcut::Value::new(query::DataType::from(2)));
        let q = query::Query::new(&[true, true, true],
                                  vec![shortcut::Condition {
                                           column: 0,
                                           cmp: val,
                                       }]);

        let hits = j.query(Some(&q));
        assert_eq!(hits.len(), 1);
        assert!(hits.iter()
            .any(|r| r[0] == 2.into() && r[1] == "b".into() && r[2] == "z".into()));

        // query using field from left
        let val = shortcut::Comparison::Equal(shortcut::Value::new(query::DataType::from("a")));
        let q = query::Query::new(&[true, true, true],
                                  vec![shortcut::Condition {
                                           column: 1,
                                           cmp: val,
                                       }]);

        let hits = j.query(Some(&q));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "x".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "y".into()));

        // query using field from right
        let val = shortcut::Comparison::Equal(shortcut::Value::new(query::DataType::from("z")));
        let q = query::Query::new(&[true, true, true],
                                  vec![shortcut::Condition {
                                           column: 2,
                                           cmp: val,
                                       }]);

        let hits = j.query(Some(&q));
        assert_eq!(hits.len(), 1);
        assert!(hits.iter()
            .any(|r| r[0] == 2.into() && r[1] == "b".into() && r[2] == "z".into()));
    }

    #[test]
    fn it_queries_left() {
        let (j, _, _) = setup(true);

        // do a full query, which should return product of left + right:
        // [ax, ay, bz, c+None]
        let hits = j.query(None);
        assert_eq!(hits.len(), 4);
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "x".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 1.into() && r[1] == "a".into() && r[2] == "y".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 2.into() && r[1] == "b".into() && r[2] == "z".into()));
        assert!(hits.iter()
            .any(|r| r[0] == 3.into() && r[1] == "c".into() && r[2] == query::DataType::None));
    }

    #[test]
    fn it_suggests_indices() {
        use std::collections::HashMap;
        let (j, l, r) = setup(false);
        let hm: HashMap<_, _> =
            vec![(l, vec![0]), // join column for left
                 (r, vec![0]), // join column for right
                 (2.into(), vec![0]) /* output column that is used as join column */]
                .into_iter()
                .collect();
        assert_eq!(j.node().suggest_indexes(2.into()), hm);
    }

    #[test]
    fn it_resolves() {
        let (j, l, r) = setup(false);
        assert_eq!(j.node().resolve(0), Some(vec![(l, 0)]));
        assert_eq!(j.node().resolve(1), Some(vec![(l, 1)]));
        assert_eq!(j.node().resolve(2), Some(vec![(r, 1)]));
    }
}
