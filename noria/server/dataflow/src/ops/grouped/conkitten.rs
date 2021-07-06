//! Kinda (s)crappy group_concat() implementation

use crate::node::Node;
use crate::ops::grouped::aggregate::SqlType;
use crate::ops::grouped::{GroupedOperation, GroupedOperator};
use crate::prelude::*;
use common::{DataType, Record};
use launchpad::Indices;
use noria::{internal, invariant_eq};
use serde_derive::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::fmt::Write;

/// The last stored state for a given group.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct LastState {
    /// The string representation we last emitted for this group.
    string_repr: String,
    /// A list of vectors (one for each source column, in order) containing the actual data.
    data_for_source_cols: Vec<Vec<DataType>>,
}

impl LastState {
    /// Set up a `LastState` for a group, making an empty vector for each source column.
    fn make(num_source_cols: usize) -> Self {
        Self {
            string_repr: "".try_into().unwrap(),
            data_for_source_cols: std::iter::repeat(vec![]).take(num_source_cols).collect(),
        }
    }
}

/// `GroupConkitten` partially implements the `GROUP_CONCAT` SQL aggregate function, which
/// aggregates a set of arbitrary `DataType`s into a string representation separated by
/// a user-defined separator.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GroupConkitten {
    /// Which columns (in order) to aggregate.
    source_cols: Vec<usize>,
    /// The columns to group by, which are just all columns not aggregated by.
    ///
    /// This is computed automatically on `setup()`.
    group_by: Vec<usize>,
    /// The user-defined separator.
    separator: String,
    /// Cached state for each group (set of data corresponding to the columns of `group_by`).
    last_state: RefCell<HashMap<Vec<DataType>, LastState>>,
}

fn concat_fmt<F: Write>(f: &mut F, dt: &DataType) -> ReadySetResult<()> {
    match dt {
        DataType::Text(..) | DataType::TinyText(..) => {
            let text: &str = <&str>::try_from(dt)?;
            write!(f, "{}", text).unwrap();
        }
        x => write!(f, "{}", x).unwrap(),
    }
    Ok(())
}

impl GroupConkitten {
    /// Construct a new `GroupConkitten`, aggregating the provided `source_cols` and separating
    /// aggregated data with the provided `separator`.
    pub fn new(
        src: NodeIndex,
        source_cols: Vec<usize>,
        separator: String,
    ) -> ReadySetResult<GroupedOperator<GroupConkitten>> {
        Ok(GroupedOperator::new(
            src,
            GroupConkitten {
                source_cols,
                group_by: vec![],
                separator,
                last_state: RefCell::new(HashMap::new()),
            },
        ))
    }
}

pub struct KittenDiff {
    record: Record,
    group_by: Vec<DataType>,
}

impl GroupedOperation for GroupConkitten {
    type Diff = KittenDiff;

    fn setup(&mut self, parent: &Node) -> ReadySetResult<()> {
        let num_cols = parent.fields().len();
        let mut group_by = HashSet::new();
        // We group by all columns that aren't involved in the aggregation, so insert all columns
        // and then remove the ones we aggregate.
        group_by.extend(0..num_cols);

        for sc in self.source_cols.iter() {
            if !group_by.remove(sc) {
                // TODO(eta): check the global_addr is actually set so we don't just turn this
                //            into an unreachable
                internal!(
                    "tried to reference invalid column {} in group_concat (of node {})",
                    sc,
                    parent.global_addr().index()
                );
            }
        }

        self.group_by = group_by.into_iter().collect();
        Ok(())
    }

    fn group_by(&self) -> &[usize] {
        &self.group_by
    }

    fn to_diff(&self, record: &[DataType], is_positive: bool) -> ReadySetResult<Self::Diff> {
        let data = record
            .cloned_indices(self.source_cols.iter().copied())
            .map_err(|_| ReadySetError::InvalidRecordLength)?;
        // We need this to figure out which state to use.
        let group_by = record
            .cloned_indices(self.group_by.iter().cloned())
            .map_err(|_| ReadySetError::InvalidRecordLength)?;
        Ok(KittenDiff {
            record: if is_positive {
                Record::Positive(data)
            } else {
                Record::Negative(data)
            },
            group_by,
        })
    }

    fn apply(
        &self,
        current: Option<&DataType>,
        diffs: &mut dyn Iterator<Item = Self::Diff>,
    ) -> ReadySetResult<DataType> {
        let current: Option<&str> = current
            .filter(|dt| matches!(dt, &DataType::Text(..) | &DataType::TinyText(..)))
            .and_then(|dt| <&str>::try_from(dt).ok());

        let first_diff = diffs
            .next()
            .ok_or_else(|| internal_err("group_concat got no diffs"))?;
        let group = first_diff.group_by.clone();

        let mut ls = self.last_state.borrow_mut().remove(&group);
        let mut prev_state = match current {
            #[allow(clippy::unwrap_used)] // check for is_some() before unwrapping
            Some(text) if ls.is_some() && text == ls.as_ref().unwrap().string_repr => {
                // if state matches, use it
                ls.take().unwrap()
            }
            // if state doesn't match, need to recreate it
            Some(_) => {
                return Err(ReadySetError::GroupedStateLost);
            }
            // if we're recreating or this is the first record for the group, make a new state
            None => LastState::make(self.source_cols.len()),
        };
        for KittenDiff { record, group_by } in Some(first_diff).into_iter().chain(diffs.into_iter())
        {
            invariant_eq!(group_by, group);

            let (data, positive_p) = record.extract();
            for (i, dt) in data.into_iter().enumerate() {
                let col_state = prev_state.data_for_source_cols.get_mut(i).ok_or_else(|| {
                    internal_err("group_concat received overlong data for previous col_state")
                })?;
                if positive_p {
                    col_state.push(dt);
                } else {
                    let item_pos = col_state.iter().rposition(|x| x == &dt).ok_or_else(|| {
                        internal_err(format!(
                            "group_concat couldn't remove {:?} from {:?}",
                            dt, col_state
                        ))
                    })?;
                    col_state.remove(item_pos);
                }
            }
        }
        // what I *really* want here is Haskell's "intercalate" ~eta
        let mut out_str = String::new();
        for (i, data) in prev_state.data_for_source_cols.iter().enumerate() {
            for (j, piece) in data.iter().enumerate() {
                // TODO(eta): not unwrap, maybe
                concat_fmt(&mut out_str, piece)?;
                if !(j == data.len() - 1 && i == prev_state.data_for_source_cols.len() - 1) {
                    write!(&mut out_str, "{}", self.separator).unwrap();
                }
            }
        }
        prev_state.string_repr = out_str.clone();
        self.last_state.borrow_mut().insert(group, prev_state);
        DataType::try_from(out_str)
    }

    fn description(&self, detailed: bool) -> String {
        if !detailed {
            return "CONCAT2".try_into().unwrap();
        }

        format!(
            "||({:?}, {:?}) γ{:?}",
            self.source_cols, self.separator, self.group_by
        )
    }

    fn over_columns(&self) -> Vec<usize> {
        self.source_cols.clone()
    }

    fn output_col_type(&self) -> Option<SqlType> {
        Some(nom_sql::SqlType::Text)
    }

    fn empty_value(&self) -> Option<DataType> {
        // It is safe to convert an empty String into a DataType, so it's
        // safe to unwrap.
        #[allow(clippy::unwrap_used)]
        Some(DataType::try_from("").unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ops;
    use std::convert::TryInto;

    fn setup(mat: bool) -> ops::test::MockGraph {
        let mut g = ops::test::MockGraph::new();
        let s = g.add_base("source", &["x", "y"]);

        let c = GroupConkitten::new(s.as_global(), vec![1], String::from("#")).unwrap();

        g.set_op("concat", &["x", "ys"], c, mat);
        g
    }

    #[test]
    fn it_describes() {
        let c = setup(true);
        assert_eq!(c.node().description(true), "||([1], \"#\") γ[0]",);
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn it_forwards() {
        let mut c = setup(true);

        let u: Record = vec![1.into(), 1.into()].into();

        // first row for a group should emit +"1" for that group
        let rs = c.narrow_one(u, true);
        assert_eq!(rs.len(), 1);
        let mut rs = rs.into_iter();

        match rs.next().unwrap() {
            Record::Positive(r) => {
                assert_eq!(r[0], 1.into());
                assert_eq!(r[1], "1".try_into().unwrap());
            }
            _ => unreachable!(),
        }

        let u: Record = vec![2.into(), 2.into()].into();

        // first row for a second group should emit +"2" for that new group
        let rs = c.narrow_one(u, true);
        assert_eq!(rs.len(), 1);
        let mut rs = rs.into_iter();

        match rs.next().unwrap() {
            Record::Positive(r) => {
                assert_eq!(r[0], 2.into());
                assert_eq!(r[1], "2".try_into().unwrap());
            }
            _ => unreachable!(),
        }

        let u: Record = vec![1.into(), 2.into()].into();

        // second row for a group should emit -"1" and +"1#2"
        let rs = c.narrow_one(u, true);
        eprintln!("{:?}", rs);
        assert_eq!(rs.len(), 2);
        let mut rs = rs.into_iter();

        match rs.next().unwrap() {
            Record::Negative(r) => {
                assert_eq!(r[0], 1.into());
                assert_eq!(r[1], "1".try_into().unwrap());
            }
            _ => unreachable!(),
        }
        match rs.next().unwrap() {
            Record::Positive(r) => {
                assert_eq!(r[0], 1.into());
                assert_eq!(r[1], "1#2".try_into().unwrap());
            }
            _ => unreachable!(),
        }

        let u = (vec![1.into(), 1.into()], false);

        // negative row for a group should emit -"1#2" and +"2"
        let rs = c.narrow_one_row(u, true);
        assert_eq!(rs.len(), 2);
        let mut rs = rs.into_iter();

        match rs.next().unwrap() {
            Record::Negative(r) => {
                assert_eq!(r[0], 1.into());
                assert_eq!(r[1], "1#2".try_into().unwrap());
            }
            _ => unreachable!(),
        }
        match rs.next().unwrap() {
            Record::Positive(r) => {
                assert_eq!(r[0], 1.into());
                assert_eq!(r[1], "2".try_into().unwrap());
            }
            _ => unreachable!(),
        }

        let u = vec![
            // remove non-existing
            // (vec![1.into(), 1.into()], false),
            // add old
            (vec![1.into(), 1.into()], true),
            // add duplicate
            (vec![1.into(), 2.into()], true),
            (vec![2.into(), 2.into()], false),
            (vec![2.into(), 3.into()], true),
            (vec![2.into(), 2.into()], true),
            (vec![2.into(), 1.into()], true),
            // new group
            (vec![3.into(), 3.into()], true),
        ];

        // multiple positives and negatives should update aggregation value by appropriate amount
        let rs = c.narrow_one(u, true);
        assert_eq!(rs.len(), 5); // one - and one + for each group, except last (new) group
                                 // group 1 had [2], now has [1,2]
        assert!(rs.iter().any(|r| if let Record::Negative(ref r) = *r {
            if r[0] == 1.into() {
                assert_eq!(r[1], "2".try_into().unwrap());
                true
            } else {
                false
            }
        } else {
            false
        }));
        assert!(rs.iter().any(|r| if let Record::Positive(ref r) = *r {
            if r[0] == 1.into() {
                assert_eq!(r[1], "2#1#2".try_into().unwrap());
                true
            } else {
                false
            }
        } else {
            false
        }));
        // group 2 was [2], is now [1,2,3]
        assert!(rs.iter().any(|r| if let Record::Negative(ref r) = *r {
            if r[0] == 2.into() {
                assert_eq!(r[1], "2".try_into().unwrap());
                true
            } else {
                false
            }
        } else {
            false
        }));
        assert!(rs.iter().any(|r| if let Record::Positive(ref r) = *r {
            if r[0] == 2.into() {
                assert_eq!(r[1], "3#2#1".try_into().unwrap());
                true
            } else {
                false
            }
        } else {
            false
        }));
        // group 3 was [], is now [3]
        assert!(rs.iter().any(|r| if let Record::Positive(ref r) = *r {
            if r[0] == 3.into() {
                assert_eq!(r[1], "3".try_into().unwrap());
                true
            } else {
                false
            }
        } else {
            false
        }));
    }

    #[test]
    fn it_suggests_indices() {
        let me = 1.into();
        let c = setup(false);
        let idx = c.node().suggest_indexes(me);

        // should only add index on own columns
        assert_eq!(idx.len(), 1);
        assert!(idx.contains_key(&me));

        // should only index on the group-by column
        assert_eq!(idx[&me], Index::hash_map(vec![0]));
    }

    #[test]
    fn it_resolves() {
        let c = setup(false);
        assert_eq!(
            c.node().resolve(0).unwrap(),
            Some(vec![(c.narrow_base_id().as_global(), 0)])
        );
        assert_eq!(c.node().resolve(1).unwrap(), None);
    }
}
