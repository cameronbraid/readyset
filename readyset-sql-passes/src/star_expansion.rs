use std::collections::HashMap;
use std::mem;

use nom_sql::analysis::visit::{self, Visitor};
use nom_sql::{Column, Expr, FieldDefinitionExpr, SelectStatement, SqlIdentifier, SqlQuery, Table};
use readyset_errors::{ReadySetError, ReadySetResult};

use crate::util::{self, join_clause_tables};

pub trait StarExpansion: Sized {
    /// Expand all `*` column references in the query given a map from tables to the lists of
    /// columns in those tables
    fn expand_stars(
        self,
        table_columns: &HashMap<Table, Vec<SqlIdentifier>>,
    ) -> ReadySetResult<Self>;
}

struct ExpandStarsVisitor<'schema> {
    table_columns: &'schema HashMap<Table, Vec<SqlIdentifier>>,
}

impl<'ast, 'schema> Visitor<'ast> for ExpandStarsVisitor<'schema> {
    type Error = ReadySetError;

    fn visit_select_statement(
        &mut self,
        select_statement: &'ast mut SelectStatement,
    ) -> Result<(), Self::Error> {
        visit::walk_select_statement(self, select_statement)?;

        let fields = mem::take(&mut select_statement.fields);
        let subquery_schemas =
            util::subquery_schemas(&select_statement.ctes, &select_statement.join);

        let expand_table = |table: Table| -> ReadySetResult<_> {
            Ok(if table.schema.is_none() {
                // Can only reference subqueries with tables that don't have a schema
                subquery_schemas.get(&table.name).cloned()
            } else {
                None
            }
            .or_else(|| self.table_columns.get(&table).map(|fs| fs.iter().collect()))
            .ok_or_else(|| ReadySetError::TableNotFound {
                name: table.name.clone().into(),
                schema: table.schema.clone().map(Into::into),
            })?
            .into_iter()
            .map(move |f| FieldDefinitionExpr::Expr {
                expr: Expr::Column(Column {
                    table: Some(table.clone()),
                    name: f.clone(),
                }),
                alias: None,
            }))
        };

        for field in fields {
            match field {
                FieldDefinitionExpr::All => {
                    for table_expr in &select_statement.tables {
                        for field in expand_table(table_expr.table.clone())? {
                            select_statement.fields.push(field);
                        }
                    }
                    for table_expr in select_statement.join.iter().flat_map(join_clause_tables) {
                        for field in expand_table(table_expr.table.clone())? {
                            select_statement.fields.push(field);
                        }
                    }
                }
                FieldDefinitionExpr::AllInTable(t) => {
                    for field in expand_table(t)? {
                        select_statement.fields.push(field);
                    }
                }
                e @ FieldDefinitionExpr::Expr { .. } => {
                    select_statement.fields.push(e);
                }
            }
        }

        Ok(())
    }
}

impl StarExpansion for SelectStatement {
    fn expand_stars(
        mut self,
        table_columns: &HashMap<Table, Vec<SqlIdentifier>>,
    ) -> ReadySetResult<Self> {
        let mut visitor = ExpandStarsVisitor { table_columns };
        visitor.visit_select_statement(&mut self)?;
        Ok(self)
    }
}

impl StarExpansion for SqlQuery {
    fn expand_stars(
        self,
        write_schemas: &HashMap<Table, Vec<SqlIdentifier>>,
    ) -> ReadySetResult<Self> {
        Ok(match self {
            SqlQuery::Select(sq) => SqlQuery::Select(sq.expand_stars(write_schemas)?),
            _ => self,
        })
    }
}

#[cfg(test)]
mod tests {
    use maplit::hashmap;
    use nom_sql::{parse_query, Dialect};

    use super::StarExpansion;

    macro_rules! expands_stars {
	    ($source: expr, $expected: expr, schema: {$($schema:tt)*}) => {{
            let q = parse_query(Dialect::MySQL, $source).unwrap();
            let expected = parse_query(Dialect::MySQL, $expected).unwrap();
            let schema = hashmap!($($schema)*);
            let res = q.expand_stars(&schema).unwrap();
            assert_eq!(res, expected, "{} != {}", res, expected);
        }};
    }

    #[test]
    fn single_table() {
        expands_stars!(
            "SELECT * FROM PaperTag",
            "SELECT PaperTag.paper_id, PaperTag.tag_id FROM PaperTag",
            schema: {
                "PaperTag".into() => vec!["paper_id".into(), "tag_id".into()]
            }
        );
    }

    #[test]
    fn multiple_tables() {
        expands_stars!(
            "SELECT * FROM PaperTag, Users",
            "SELECT PaperTag.paper_id, PaperTag.tag_id, Users.uid, Users.name FROM PaperTag, Users",
            schema: {
                "PaperTag".into() => vec!["paper_id".into(), "tag_id".into()],
                "Users".into() => vec!["uid".into(), "name".into()],
            }
        );
    }

    #[test]
    fn table_stars() {
        expands_stars!(
            "SELECT Users.*, PaperTag.* FROM PaperTag, Users",
            "SELECT Users.uid, Users.name, PaperTag.paper_id, PaperTag.tag_id FROM PaperTag, Users",
            schema: {
                "PaperTag".into() => vec!["paper_id".into(), "tag_id".into()],
                "Users".into() => vec!["uid".into(), "name".into()],
            }
        );
    }

    #[test]
    fn in_cte() {
        expands_stars!(
            "WITH users AS (SELECT Users.* FROM Users) SELECT uid FROM users",
            "WITH users AS (SELECT Users.uid, Users.name FROM Users) SELECT uid FROM users",
            schema: {
                "Users".into() => vec!["uid".into(), "name".into()]
            }
        );
    }

    #[test]
    fn referencing_cte() {
        expands_stars!(
            "WITH users AS (SELECT Users.* FROM Users) SELECT * FROM users",
            "WITH users AS (SELECT Users.uid, Users.name FROM Users) SELECT users.uid, users.name FROM users",
            schema: {
                "Users".into() => vec!["uid".into(), "name".into()]
            }
        );
    }

    #[test]
    fn referencing_cte_shadowing_table() {
        expands_stars!(
            "WITH t2 AS (SELECT * FROM t1) SELECT * FROM t2",
            "WITH t2 AS (SELECT t1.a, t1.b FROM t1) SELECT t2.a, t2.b FROM t2",
            schema: {
                "t1".into() => vec!["a".into(), "b".into()],
                "t2".into() => vec!["c".into(), "d".into()],
            }
        )
    }

    #[test]
    fn in_subquery() {
        expands_stars!(
            "SELECT uid FROM PaperTag JOIN (SELECT Users.* FROM Users) users On paper_id = uid",
            "SELECT uid FROM PaperTag JOIN (SELECT Users.uid, Users.name FROM Users) users On paper_id = uid",
            schema: {
                "PaperTag".into() => vec!["paper_id".into(), "tag_id".into()],
                "Users".into() => vec!["uid".into(), "name".into()]
            }
        );
    }

    #[test]
    fn referencing_subquery() {
        expands_stars!(
            "SELECT users.* FROM PaperTag JOIN (SELECT Users.* FROM Users) users On paper_id = uid",
            "SELECT users.uid, users.name FROM PaperTag JOIN (SELECT Users.uid, Users.name FROM Users) users On paper_id = uid",
            schema: {
                "Users".into() => vec!["uid".into(), "name".into()]
            }
        );
    }

    #[test]
    fn simple_join() {
        expands_stars!(
            "SELECT * FROM t1 JOIN t2 on t1.a = t2.b",
            "SELECT t1.a, t2.b FROM t1 JOIN t2 on t1.a = t2.b",
            schema: {
                "t1".into() => vec!["a".into()],
                "t2".into() => vec!["b".into()],
            }
        );
    }
}