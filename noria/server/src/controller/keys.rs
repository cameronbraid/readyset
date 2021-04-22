use dataflow::prelude::*;
use noria::ReadySetError;
use petgraph;

use std::collections::HashMap;

// TODO: rewrite as iterator
pub fn provenance_of<F>(
    graph: &Graph,
    node: NodeIndex,
    columns: &[usize],
    mut on_join: F,
) -> Result<Vec<Vec<(NodeIndex, Vec<Option<usize>>)>>, ReadySetError>
where
    F: FnMut(NodeIndex, &[Option<usize>], &[NodeIndex]) -> Result<Option<NodeIndex>, ReadySetError>,
{
    let path = vec![(node, columns.iter().map(|&v| Some(v)).collect())];
    trace(graph, &mut on_join, path)
}

fn trace<F>(
    graph: &Graph,
    on_join: &mut F,
    mut path: Vec<(NodeIndex, Vec<Option<usize>>)>,
) -> Result<Vec<Vec<(NodeIndex, Vec<Option<usize>>)>>, ReadySetError>
where
    F: FnMut(NodeIndex, &[Option<usize>], &[NodeIndex]) -> Result<Option<NodeIndex>, ReadySetError>,
{
    // figure out what node/column we're looking up
    let (node, columns) = path.last().cloned().unwrap();
    let cols = columns.len();

    let parents: Vec<_> = graph
        .neighbors_directed(node, petgraph::EdgeDirection::Incoming)
        .collect();

    if parents.is_empty() {
        // this path reached the source node.
        // but we should have stopped at base nodes above...
        unreachable!();
    }

    let n = &graph[node];

    // have we reached a base node?
    if n.is_base() {
        return Ok(vec![path]);
    }

    // we know all non-internal nodes use an identity mapping
    if !n.is_internal() {
        let parent = parents[0];
        path.push((parent, columns));
        return trace(graph, on_join, path);
    }

    // if all our inputs are None, our job is trivial
    // we just go trace back to all ancestors
    if columns.iter().all(Option::is_none) {
        // except if we're a join and on_join says to only walk through one...
        if n.is_internal() && n.is_join() {
            let idk = vec![None; cols];
            if let Some(parent) = on_join(node, &idk[..], &parents[..])? {
                path.push((parent, idk));
                return trace(graph, on_join, path);
            }
        }

        let mut paths = Vec::with_capacity(parents.len());
        for p in parents {
            let mut path = path.clone();
            path.push((p, vec![None; cols]));
            paths.extend(trace(graph, on_join, path)?);
        }
        return Ok(paths);
    }

    // try to resolve the currently selected columns
    let mut resolved = columns
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| c.map(|c| (i, c)))
        .map(|(i, c)| Ok((i, n.parent_columns(c)?)))
        .collect::<Result<Vec<(usize, Vec<(NodeIndex, Option<usize>)>)>, ReadySetError>>()?
        .iter()
        .flat_map(|(i, origins)| {
            assert!(!origins.is_empty());
            origins.into_iter().map(move |o| (i, o))
        })
        .fold(
            HashMap::new(),
            |mut by_ancestor, (coli, (ancestor, column))| {
                {
                    let resolved = by_ancestor
                        .entry(*ancestor)
                        .or_insert_with(|| vec![None; cols]);
                    resolved[*coli] = *column;
                }
                by_ancestor
            },
        );
    assert!(!resolved.is_empty(), "Some(col) resolved into no ancestors");

    // are any of the columns generated?
    if let Some(columns) = resolved.remove(&node) {
        // some are, so at this point we know we'll need to yield None for those columns all the
        // way back to the root of the graph.

        // resolving to Some on self makes no sense...
        assert!(columns.iter().all(Option::is_none));

        if parents.len() != 1 {
            // TODO: we have a join-like thing, so we'd need to call on_join
            // like in the case of all our inputs being None above.
            unimplemented!();
        }

        let mut paths = Vec::with_capacity(parents.len());
        for p in parents {
            let mut path = path.clone();
            path.push((p, resolved.remove(&p).unwrap_or_else(|| vec![None; cols])));
            paths.extend(trace(graph, on_join, path)?);
        }
        return Ok(paths);
    }

    // no, it resolves to at least one parent column
    // if there is only one parent, we can step right to that
    if parents.len() == 1 {
        let parent = parents[0];
        let resolved = resolved.remove(&parent).unwrap();
        path.push((parent, resolved));
        return trace(graph, on_join, path);
    }

    // there are multiple parents.
    // this means we are either a union or a join.
    // let's deal with the union case first.
    // in unions, all keys resolve to more than one parent.
    if !n.is_join() {
        // all columns come from all parents
        assert_eq!(parents.len(), resolved.len());
        // traverse up all the paths
        let mut paths = Vec::with_capacity(parents.len());
        for (parent, columns) in resolved {
            let mut path = path.clone();
            path.push((parent, columns));
            paths.extend(trace(graph, on_join, path)?);
        }
        return Ok(paths);
    }

    // okay, so this is a join. it's up to the on_join function to tell us whether to walk up *all*
    // the parents of the join, or just one of them. let's ask.
    // TODO: provide an early-termination mechanism?
    match on_join(node, &columns[..], &parents[..])? {
        None => {
            // our caller wants information about all our parents.
            // since the column we're chasing only follows a single path through a join (unless it
            // is a join key, which we don't yet handle), we need to produce (_, None) for all the
            // others.
            let mut paths = Vec::with_capacity(parents.len());
            for parent in parents {
                let mut path = path.clone();
                path.push((
                    parent,
                    resolved.remove(&parent).unwrap_or_else(|| vec![None; cols]),
                ));
                paths.extend(trace(graph, on_join, path)?);
            }
            Ok(paths)
        }
        Some(parent) => {
            // our caller only cares about *one* parent.
            // hopefully we can give key information about that parent
            path.push((
                parent,
                resolved.remove(&parent).unwrap_or_else(|| vec![None; cols]),
            ));
            trace(graph, on_join, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataflow::node;
    use dataflow::ops;

    static EMPTY_ON_JOIN: fn(
        NodeIndex,
        &[Option<usize>],
        &[NodeIndex],
    ) -> Result<Option<NodeIndex>, ReadySetError> = |_, _, _| Ok(None);

    fn bases() -> (Graph, NodeIndex, NodeIndex) {
        let mut g = petgraph::Graph::new();
        let src = g.add_node(node::Node::new(
            "source",
            &["because-type-inference"],
            node::special::Source,
        ));

        let a = g.add_node(node::Node::new(
            "a",
            &["a1", "a2"],
            node::special::Base::default(),
        ));
        g.add_edge(src, a, ());

        let b = g.add_node(node::Node::new(
            "b",
            &["b1", "b2"],
            node::special::Base::default(),
        ));
        g.add_edge(src, b, ());

        (g, a, b)
    }

    #[test]
    fn base_trace() {
        let (g, a, b) = bases();
        assert_eq!(
            provenance_of(&g, a, &[0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(a, vec![Some(0)])]]
        );
        assert_eq!(
            provenance_of(&g, b, &[0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(b, vec![Some(0)])]]
        );

        // multicol
        assert_eq!(
            provenance_of(&g, a, &[0, 1], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(a, vec![Some(0), Some(1)])]]
        );
        assert_eq!(
            provenance_of(&g, a, &[1, 0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(a, vec![Some(1), Some(0)])]]
        );
    }

    #[test]
    fn internal_passthrough() {
        let (mut g, a, _) = bases();

        let x = g.add_node(node::Node::new("x", &["x1", "x2"], node::special::Ingress));
        g.add_edge(a, x, ());

        assert_eq!(
            provenance_of(&g, x, &[0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(x, vec![Some(0)]), (a, vec![Some(0)])]]
        );
        assert_eq!(
            provenance_of(&g, x, &[0, 1], EMPTY_ON_JOIN).unwrap(),
            vec![vec![
                (x, vec![Some(0), Some(1)]),
                (a, vec![Some(0), Some(1)]),
            ]]
        );
    }

    #[test]
    fn col_reorder() {
        let (mut g, a, _) = bases();

        let x = g.add_node(node::Node::new(
            "x",
            &["x2", "x1"],
            ops::NodeOperator::Project(ops::project::Project::new(a, &[1, 0], None, None)),
        ));
        g.add_edge(a, x, ());

        assert_eq!(
            provenance_of(&g, x, &[0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(x, vec![Some(0)]), (a, vec![Some(1)])]]
        );
        assert_eq!(
            provenance_of(&g, x, &[0, 1], EMPTY_ON_JOIN).unwrap(),
            vec![vec![
                (x, vec![Some(0), Some(1)]),
                (a, vec![Some(1), Some(0)]),
            ]]
        );
    }

    #[test]
    fn generated_cols() {
        use std::convert::TryFrom;

        let (mut g, a, _) = bases();

        let x = g.add_node(node::Node::new(
            "x",
            &["x1", "foo"],
            ops::NodeOperator::Project(ops::project::Project::new(
                a,
                &[0],
                Some(vec![DataType::try_from(3.14).unwrap()]),
                None,
            )),
        ));
        g.add_edge(a, x, ());

        assert_eq!(
            provenance_of(&g, x, &[0], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(x, vec![Some(0)]), (a, vec![Some(0)])]]
        );
        assert_eq!(
            provenance_of(&g, x, &[1], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(x, vec![Some(1)]), (a, vec![None])]]
        );
        assert_eq!(
            provenance_of(&g, x, &[0, 1], EMPTY_ON_JOIN).unwrap(),
            vec![vec![(x, vec![Some(0), Some(1)]), (a, vec![Some(0), None])]]
        );
    }

    #[test]
    fn union_straight() {
        let (mut g, a, b) = bases();

        let x = g.add_node(node::Node::new(
            "x",
            &["x1", "x2"],
            ops::NodeOperator::Union(ops::union::Union::new(
                vec![(a, vec![0, 1]), (b, vec![0, 1])].into_iter().collect(),
            )),
        ));
        g.add_edge(a, x, ());
        g.add_edge(b, x, ());

        let mut paths = provenance_of(&g, x, &[0], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(0)]), (a, vec![Some(0)])],
                vec![(x, vec![Some(0)]), (b, vec![Some(0)])],
            ]
        );
        let mut paths = provenance_of(&g, x, &[0, 1], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(0), Some(1)]), (a, vec![Some(0), Some(1)])],
                vec![(x, vec![Some(0), Some(1)]), (b, vec![Some(0), Some(1)])],
            ]
        );
    }

    #[test]
    fn join_all() {
        let (mut g, a, b) = bases();

        let x = g.add_node(node::Node::new(
            "x",
            &["a1", "a2b1", "b2"],
            ops::NodeOperator::Join(ops::join::Join::new(
                a,
                b,
                ops::join::JoinType::Inner,
                vec![
                    ops::join::JoinSource::L(0),
                    ops::join::JoinSource::B(1, 0),
                    ops::join::JoinSource::R(1),
                ],
            )),
        ));
        g.add_edge(a, x, ());
        g.add_edge(b, x, ());

        let mut paths = provenance_of(&g, x, &[0], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(0)]), (a, vec![Some(0)])],
                vec![(x, vec![Some(0)]), (b, vec![None])],
            ]
        );
        let mut paths = provenance_of(&g, x, &[1], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(1)]), (a, vec![Some(1)])],
                vec![(x, vec![Some(1)]), (b, vec![Some(0)])],
            ]
        );
        let mut paths = provenance_of(&g, x, &[2], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(2)]), (a, vec![None])],
                vec![(x, vec![Some(2)]), (b, vec![Some(1)])],
            ]
        );
        let mut paths = provenance_of(&g, x, &[0, 1], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(0), Some(1)]), (a, vec![Some(0), Some(1)])],
                vec![(x, vec![Some(0), Some(1)]), (b, vec![None, Some(0)])],
            ]
        );
        let mut paths = provenance_of(&g, x, &[1, 2], EMPTY_ON_JOIN).unwrap();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                vec![(x, vec![Some(1), Some(2)]), (a, vec![Some(1), None])],
                vec![(x, vec![Some(1), Some(2)]), (b, vec![Some(0), Some(1)])],
            ]
        );
    }
}
