use flow;
use petgraph::graph::NodeIndex;
use flow::prelude::*;

macro_rules! broadcast {
    ($from:expr, $handoffs:ident, $m:expr, $children:expr) => {{
        let c = $children;
        let mut m = $m;
        m.from = $from;
        let mut m = Some(m); // so we can .take() below
        for (i, to) in c.iter().enumerate() {
            let u = if i == c.len() - 1 {
                m.take()
            } else {
                m.clone()
            };

            $handoffs.get_mut(to).unwrap().push_back(u.unwrap());
        }
    }}
}

pub struct NodeDescriptor {
    pub index: NodeIndex,
    pub inner: Node,
    pub children: Vec<NodeAddress>,
}

pub enum FinalProcessingResult {
    Done(Packet),
    NeedReplay {
        node: NodeAddress,
        key: Vec<DataType>,
        was: Packet,
    },
}

impl NodeDescriptor {
    pub fn new(graph: &mut Graph, node: NodeIndex) -> Self {
        use petgraph;

        let inner = graph.node_weight_mut(node).unwrap().take();
        let children: Vec<_> = graph.neighbors_directed(node, petgraph::EdgeDirection::Outgoing)
            .filter(|&c| graph[c].domain() == inner.domain())
            .map(|ni| graph[ni].addr())
            .collect();

        NodeDescriptor {
            index: node,
            inner: inner,
            children: children,
        }
    }

    pub fn process(&mut self,
                   mut m: Packet,
                   state: &mut StateMap,
                   nodes: &DomainNodes,
                   swap: bool)
                   -> FinalProcessingResult {

        use flow::payload::TransactionState;
        let addr = self.addr();
        match *self.inner {
            flow::node::Type::Ingress => {
                if let Err(key) = materialize(m.data(), state.get_mut(addr.as_local())) {
                    return FinalProcessingResult::NeedReplay {
                               node: addr,
                               was: m,
                               key: vec![key],
                           };
                }
                FinalProcessingResult::Done(m)
            }
            flow::node::Type::Reader(ref mut w, ref r) => {
                if let Some(ref mut state) = *w {
                    let r = r.state.as_ref().unwrap();
                    // make sure we don't fill a partial materialization
                    // hole with incomplete (i.e., non-replay) state.
                    if m.is_regular() && r.is_partial() {
                        let key = r.key();
                        m.map_data(|mut data| {
                            data.retain(|row| {
                                match r.find_and(&row[key], |_| ()) {
                                    Ok((None, _)) => {
                                        // row would miss in partial state.
                                        // leave it blank so later lookup triggers replay.
                                        false
                                    }
                                    Err(_) => unreachable!(),
                                    _ => {
                                        // state is already present,
                                        // so we can safely keep it up to date.
                                        true
                                    }
                                }
                            });
                            data
                        });
                    }

                    // it *can* happen that multiple readers miss (and thus request replay for) the
                    // same hole at the same time. we need to make sure that we ignore any such
                    // duplicated replay.
                    if !m.is_regular() && r.is_partial() {
                        let key = r.key();
                        m.map_data(|mut data| {
                            data.retain(|row| {
                                match r.find_and(&row[key], |_| ()) {
                                    Ok((None, _)) => {
                                        // filling a hole with replay -- ok
                                        true
                                    }
                                    Ok((Some(_), _)) => {
                                        // a given key should only be replayed to once!
                                        false
                                    }
                                    Err(_) => {
                                        // state has not yet been swapped, which means it's new,
                                        // which means there are no readers, which means no
                                        // requests for replays have been issued by readers, which
                                        // means no duplicates can be received.
                                        true
                                    }
                                }
                            });
                            data
                        });
                    }

                    state.add(m.data().iter().cloned());
                    if let Packet::Transaction {
                               state: TransactionState::Committed(ts, ..), ..
                           } = m {
                        state.update_ts(ts);
                    }

                    // TODO: avoid swapping if writes are empty

                    if swap {
                        state.swap();
                    }
                }

                // TODO: don't send replays to streams?

                let mut data = Some(m.take_data()); // so we can .take() for last tx
                let mut txs = r.streamers.lock().unwrap();
                let mut left = txs.len();

                // remove any channels where the receiver has hung up
                txs.retain(|tx| {
                    left -= 1;
                    if left == 0 {
                            tx.send(data.take()
                                        .unwrap()
                                        .into_iter()
                                        .map(|r| r.into())
                                        .collect())
                        } else {
                            tx.send(data.clone()
                                        .unwrap()
                                        .into_iter()
                                        .map(|r| r.into())
                                        .collect())
                        }
                        .is_ok()
                });

                // readers never have children
                FinalProcessingResult::Done(Packet::None)
            }
            flow::node::Type::Hook(ref mut h) => {
                if let &mut Some(ref mut h) = h {
                    h.on_input(m.take_data());
                } else {
                    unreachable!();
                }
                FinalProcessingResult::Done(Packet::None)
            }
            flow::node::Type::Egress { ref txs, ref tags } => {
                // send any queued updates to all external children
                let mut txs = txs.lock().unwrap();
                let txn = txs.len() - 1;

                debug_assert!(self.children.is_empty());

                // we need to find the ingress node following this egress according to the path
                // with replay.tag, and then forward this message only on the channel corresponding
                // to that ingress node.
                let replay_to = if let Packet::Replay { tag, .. } = m {
                    Some(tags.lock()
                        .unwrap()
                        .get(&tag)
                        .map(|n| *n)
                        .expect("egress node told about replay message, but not on replay path"))
                } else {
                    None
                };

                let mut m = Some(m); // so we can use .take()
                for (txi, &mut (ref globaddr, dst, ref mut tx)) in txs.iter_mut().enumerate() {
                    let mut take = txi == txn;
                    if let Some(replay_to) = replay_to.as_ref() {
                        if replay_to == globaddr {
                            take = true;
                        } else {
                            continue;
                        }
                    }

                    // avoid cloning if this is last send
                    let mut m = if take {
                        m.take().unwrap()
                    } else {
                        // we know this is a data (not a replay)
                        // because, a replay will force a take
                        m.as_ref().map(|m| m.clone_data()).unwrap()
                    };

                    m.link_mut().src = self.index.into();
                    m.link_mut().dst = dst;

                    if tx.send(m).is_err() {
                        // we must be shutting down...
                        break;
                    }

                    if take {
                        break;
                    }
                }
                debug_assert!(m.is_none());
                FinalProcessingResult::Done(Packet::None)
            }
            flow::node::Type::Internal(ref mut i) => {
                let from = m.link().src;
                let mut need_replay = None;

                let mut data_clone = None;
                m.map_data(|data| {
                    // clone if we're partially materialized; we may need to back out
                    if state.get_mut(addr.as_local()).map(|s| s.is_partial()).unwrap_or(false) {
                        data_clone = Some(data.clone());
                    }
                    match i.on_input(from, data, nodes, state) {
                        ProcessingResult::Done(rs) => rs,
                        ProcessingResult::NeedReplay { node, key, was } => {
                            need_replay = Some((node, key));
                            was
                        }
                    }
                });

                match need_replay {
                    None => {
                        if let Err(key) = materialize(m.data(), state.get_mut(addr.as_local())) {
                            m.map_data(|_| data_clone.take().unwrap());
                            return FinalProcessingResult::NeedReplay {
                                       node: addr,
                                       was: m,
                                       key: vec![key],
                                   };
                        }
                        FinalProcessingResult::Done(m)
                    }
                    Some((node, key)) => {
                        FinalProcessingResult::NeedReplay {
                            node: node,
                            key: key,
                            was: m,
                        }
                    }
                }
            }
            flow::node::Type::Source => unreachable!(),
        }
    }
}

pub fn materialize(rs: &Records, state: Option<&mut State>) -> Result<(), DataType> {
    // our output changed -- do we need to modify materialized state?
    if state.is_none() {
        // nope
        return Ok(());
    }

    // yes!
    let mut state = state.unwrap();

    // are we partially materialized?
    if state.is_partial() {
        // yes -- we need to check that we're not hitting any holes
        for r in rs.iter() {
            if let Some(cols) = state.hits_hole(r) {
                // we need a replay of this update!
                if cols.len() != 1 {
                    unimplemented!();
                }
                return Err(r[cols[0]].clone());
            }
        }
    }

    for r in rs.iter() {
        match *r {
            Record::Positive(ref r) => state.insert(r.clone()),
            Record::Negative(ref r) => state.remove(r),
            Record::DeleteRequest(..) => unreachable!(),
        }
    }

    Ok(())
}

use std::ops::Deref;
impl Deref for NodeDescriptor {
    type Target = Node;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
