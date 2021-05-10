use nom::character::complete::{multispace0, multispace1};
use nom::error::ErrorKind;
use nom::multi::many0;
use nom::{
    alt, char, complete, delimited, do_parse, named, opt, separated_nonempty_list, tag, tag_no_case,
};
use std::fmt;
use std::str;

use crate::common::{
    as_alias, field_definition_expr, field_list, schema_table_reference, sql_identifier,
    statement_terminator, table_list, unsigned_number, ws_sep_comma, FieldDefinitionExpression,
};
use crate::condition::{condition_expr, ConditionExpression};
use crate::join::{join_operator, JoinConstraint, JoinOperator, JoinRightSide};
use crate::order::{order_clause, OrderClause};
use crate::table::Table;
use crate::Column;
use nom::branch::alt;
use nom::bytes::complete::{tag, tag_no_case};
use nom::combinator::{map, opt};
use nom::sequence::{delimited, preceded, terminated, tuple};
use nom::IResult;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct GroupByClause {
    pub columns: Vec<Column>,
    pub having: Option<ConditionExpression>,
}

impl fmt::Display for GroupByClause {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "GROUP BY ")?;
        write!(
            f,
            "{}",
            self.columns
                .iter()
                .map(|c| format!("{}", c))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        if let Some(ref having) = self.having {
            write!(f, " HAVING {}", having)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct JoinClause {
    pub operator: JoinOperator,
    pub right: JoinRightSide,
    pub constraint: JoinConstraint,
}

impl fmt::Display for JoinClause {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.operator)?;
        write!(f, " {}", self.right)?;
        write!(f, " {}", self.constraint)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct LimitClause {
    pub limit: u64,
    pub offset: u64,
}

impl fmt::Display for LimitClause {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "LIMIT {}", self.limit)?;
        if self.offset > 0 {
            write!(f, " OFFSET {}", self.offset)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct CommonTableExpression {
    pub name: String,
    pub statement: SelectStatement,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SelectStatement {
    pub ctes: Vec<CommonTableExpression>,
    pub tables: Vec<Table>,
    pub distinct: bool,
    pub fields: Vec<FieldDefinitionExpression>,
    pub join: Vec<JoinClause>,
    pub where_clause: Option<ConditionExpression>,
    pub group_by: Option<GroupByClause>,
    pub order: Option<OrderClause>,
    pub limit: Option<LimitClause>,
}

impl fmt::Display for SelectStatement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SELECT ")?;
        if self.distinct {
            write!(f, "DISTINCT ")?;
        }
        write!(
            f,
            "{}",
            self.fields
                .iter()
                .map(|field| format!("{}", field))
                .collect::<Vec<_>>()
                .join(", ")
        )?;

        if !self.tables.is_empty() {
            write!(f, " FROM ")?;
            write!(
                f,
                "{}",
                self.tables
                    .iter()
                    .map(|table| format!("{}", table))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        for jc in &self.join {
            write!(f, " {}", jc)?;
        }
        if let Some(ref where_clause) = self.where_clause {
            write!(f, " WHERE ")?;
            write!(f, "{}", where_clause)?;
        }
        if let Some(ref group_by) = self.group_by {
            write!(f, " {}", group_by)?;
        }
        if let Some(ref order) = self.order {
            write!(f, " {}", order)?;
        }
        if let Some(ref limit) = self.limit {
            write!(f, " {}", limit)?;
        }
        Ok(())
    }
}

fn having_clause(i: &[u8]) -> IResult<&[u8], ConditionExpression> {
    let (remaining_input, (_, _, _, ce)) = tuple((
        multispace0,
        tag_no_case("having"),
        multispace1,
        condition_expr,
    ))(i)?;

    Ok((remaining_input, ce))
}

// Parse GROUP BY clause
pub fn group_by_clause(i: &[u8]) -> IResult<&[u8], GroupByClause> {
    let (remaining_input, (_, _, _, columns, having)) = tuple((
        multispace0,
        tag_no_case("group by"),
        multispace1,
        field_list,
        opt(having_clause),
    ))(i)?;

    Ok((remaining_input, GroupByClause { columns, having }))
}

fn offset(i: &[u8]) -> IResult<&[u8], u64> {
    let (remaining_input, (_, _, _, val)) = tuple((
        multispace0,
        tag_no_case("offset"),
        multispace1,
        unsigned_number,
    ))(i)?;

    Ok((remaining_input, val))
}

// Parse LIMIT clause
pub fn limit_clause(i: &[u8]) -> IResult<&[u8], LimitClause> {
    let (remaining_input, (_, _, _, limit, opt_offset)) = tuple((
        multispace0,
        tag_no_case("limit"),
        multispace1,
        unsigned_number,
        opt(offset),
    ))(i)?;
    let offset = opt_offset.unwrap_or(0);

    Ok((remaining_input, LimitClause { limit, offset }))
}

fn join_constraint(i: &[u8]) -> IResult<&[u8], JoinConstraint> {
    let using_clause = map(
        tuple((
            tag_no_case("using"),
            multispace1,
            delimited(
                terminated(tag("("), multispace0),
                field_list,
                preceded(multispace0, tag(")")),
            ),
        )),
        |t| JoinConstraint::Using(t.2),
    );
    let on_condition = alt((
        delimited(
            terminated(tag("("), multispace0),
            condition_expr,
            preceded(multispace0, tag(")")),
        ),
        preceded(multispace1, condition_expr),
    ));
    let on_clause = map(tuple((tag_no_case("on"), on_condition)), |t| {
        JoinConstraint::On(t.1)
    });

    alt((using_clause, on_clause))(i)
}

// Parse JOIN clause
fn join_clause(i: &[u8]) -> IResult<&[u8], JoinClause> {
    let (remaining_input, (_, _natural, operator, _, right, _, constraint)) = tuple((
        multispace0,
        opt(terminated(tag_no_case("natural"), multispace1)),
        join_operator,
        multispace1,
        join_rhs,
        multispace0,
        join_constraint,
    ))(i)?;

    Ok((
        remaining_input,
        JoinClause {
            operator,
            right,
            constraint,
        },
    ))
}

fn join_rhs(i: &[u8]) -> IResult<&[u8], JoinRightSide> {
    let nested_select = map(
        tuple((
            delimited(
                terminated(tag("("), multispace0),
                nested_selection,
                preceded(multispace0, tag(")")),
            ),
            opt(as_alias),
        )),
        |t| JoinRightSide::NestedSelect(Box::new(t.0), t.1.map(String::from)),
    );
    let nested_join = map(delimited(tag("("), join_clause, tag(")")), |nj| {
        JoinRightSide::NestedJoin(Box::new(nj))
    });
    let table = map(schema_table_reference, JoinRightSide::Table);
    let tables = map(delimited(tag("("), table_list, tag(")")), |tables| {
        JoinRightSide::Tables(tables)
    });
    alt((nested_select, nested_join, table, tables))(i)
}

// Parse WHERE clause of a selection
pub fn where_clause(i: &[u8]) -> IResult<&[u8], ConditionExpression> {
    let (remaining_input, (_, _, _, where_condition)) = tuple((
        multispace0,
        tag_no_case("where"),
        multispace1,
        condition_expr,
    ))(i)?;

    Ok((remaining_input, where_condition))
}

// Parse rule for a SQL selection query.
pub fn selection(i: &[u8]) -> IResult<&[u8], SelectStatement> {
    terminated(nested_selection, statement_terminator)(i)
}

/// The semantics of SQL natively represent the FROM clause of a query as a fully nested AST of join
/// clauses, but our AST has distinct fields for the tables and a list of joins. To be able to parse
/// parenthesized join clauses with explicit precedence such as `FROM ((t1 JOIN t2) JOIN t3)`, we
/// first parse to a tree then convert to the latter representation afterwards.
#[derive(Debug)]
enum FromClause {
    Tables(Vec<Table>),
    NestedSelect(Box<SelectStatement>, Option<String>),
    Join {
        lhs: Box<FromClause>,
        join_clause: JoinClause,
    },
}

impl FromClause {
    fn into_tables_and_joins(self) -> Result<(Vec<Table>, Vec<JoinClause>), String> {
        use FromClause::*;

        // The current representation means that a nested select on the lhs of a join is never
        // allowed
        let lhs_nested_select = Err("SELECT from subqueries is not currently supported".to_owned());

        match self {
            Tables(tables) => Ok((tables, vec![])),
            NestedSelect(_, _) => lhs_nested_select,
            Join {
                mut lhs,
                join_clause,
            } => {
                let mut joins = vec![join_clause];
                let tables = loop {
                    match *lhs {
                        Tables(tables) => break tables,
                        NestedSelect(_, _) => return lhs_nested_select,
                        Join {
                            lhs: new_lhs,
                            join_clause,
                        } => {
                            joins.push(join_clause);
                            lhs = new_lhs;
                        }
                    }
                };
                joins.reverse();
                Ok((tables, joins))
            }
        }
    }
}

named!(nested_select(&[u8]) -> FromClause, do_parse!(
    tag!("(")
        >> multispace0
        >> selection: nested_selection
        >> multispace0
        >> tag!(")")
        >> alias: opt!(as_alias)
        >> (FromClause::NestedSelect(Box::new(selection), alias.map(|s| s.to_owned())))
));

named!(from_clause_join(&[u8]) -> FromClause, do_parse!(
    multispace0
        >> lhs: nested_from_clause
        >> join_clause: join_clause
        >> multispace0
        >> (FromClause::Join {
            lhs: Box::new(lhs),
            join_clause
        })
));

named!(nested_from_clause(&[u8]) -> FromClause, alt!(
    delimited!(tag!("("), from_clause_join, tag!(")"))
        | table_list => { |ts| FromClause::Tables(ts) }
        | nested_select
));

named!(from_clause_tree(&[u8]) -> FromClause, alt!(
    delimited!(tag!("("), from_clause_join, tag!(")"))
        | from_clause_join
        | table_list => { |ts| FromClause::Tables(ts) }
        | nested_select
));

named!(from_clause(&[u8]) -> FromClause, do_parse!(
    multispace0
        >> tag_no_case!("from")
        >> multispace1
        >> from_clause: from_clause_tree
        >> (from_clause)
));

named!(cte(&[u8]) -> CommonTableExpression, do_parse!(
    name: sql_identifier
        >> multispace1
        >> complete!(tag_no_case!("as"))
        >> multispace0
        >> char!('(')
        >> multispace0
        >> statement: nested_selection
        >> multispace0
        >> char!(')')
        >> (CommonTableExpression {
            name: String::from_utf8(name.to_owned()).unwrap(),
            statement
        })
));

named!(ctes(&[u8]) -> Vec<CommonTableExpression>, do_parse!(
    complete!(tag_no_case!("with"))
        >> multispace1
        >> ctes: separated_nonempty_list!(ws_sep_comma, cte)
        >> multispace0
        >> (ctes)
));

pub fn nested_selection(i: &[u8]) -> IResult<&[u8], SelectStatement> {
    let (remaining_input, (ctes, _, _, distinct, _, fields)) = tuple((
        opt(ctes),
        tag_no_case("select"),
        multispace1,
        opt(tag_no_case("distinct")),
        multispace0,
        field_definition_expr,
    ))(i)?;

    let (remaining_input, from_clause) = opt(tuple((
        from_clause,
        many0(join_clause),
        opt(where_clause),
        opt(group_by_clause),
        opt(order_clause),
        opt(limit_clause),
    )))(remaining_input)?;

    let mut result = SelectStatement {
        ctes: ctes.unwrap_or_default(),
        distinct: distinct.is_some(),
        fields,
        ..Default::default()
    };

    if let Some((from, extra_joins, where_clause, group_by, order, limit)) = from_clause {
        let (tables, mut join) = from
            .into_tables_and_joins()
            .map_err(|_| nom::Err::Error((remaining_input, ErrorKind::Tag)))?;

        join.extend(extra_joins);

        result.tables = tables;
        result.join = join;
        result.where_clause = where_clause;
        result.group_by = group_by;
        result.order = order;
        result.limit = limit;
    }

    Ok((remaining_input, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arithmetic::{Arithmetic, ArithmeticOperator};
    use crate::column::Column;
    use crate::common::{
        BinaryOperator, FieldDefinitionExpression, ItemPlaceholder, Literal, SqlType,
    };
    use crate::condition::ConditionBase::*;
    use crate::condition::ConditionExpression::{Base, Bracketed, ComparisonOp, LogicalOp};
    use crate::condition::ConditionTree;
    use crate::order::OrderType;
    use crate::table::Table;
    use crate::{Expression, FunctionExpression};

    fn columns(cols: &[&str]) -> Vec<FieldDefinitionExpression> {
        cols.iter()
            .map(|c| FieldDefinitionExpression::from(Column::from(*c)))
            .collect()
    }

    #[test]
    fn simple_select() {
        let qstring = "SELECT id, name FROM users;";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: columns(&["id", "name"]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn select_without_table() {
        let qstring = "SELECT * FROM;";
        let res = selection(qstring.as_bytes());
        assert!(res.is_err(), "!{:?}.is_err()", res);
    }

    #[test]
    fn more_involved_select() {
        let qstring = "SELECT users.id, users.name FROM users;";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: columns(&["users.id", "users.name"]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn select_literals() {
        use crate::common::Literal;

        let qstring = "SELECT NULL, 1, \"foo\", CURRENT_TIME FROM users;";
        // TODO: doesn't support selecting literals without a FROM clause, which is still valid SQL
        //        let qstring = "SELECT NULL, 1, \"foo\";";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: vec![
                    FieldDefinitionExpression::from(Expression::Literal(Literal::Null,)),
                    FieldDefinitionExpression::from(Expression::Literal(Literal::Integer(1),)),
                    FieldDefinitionExpression::from(Expression::Literal(Literal::String(
                        "foo".to_owned()
                    ),)),
                    FieldDefinitionExpression::from(Expression::Literal(Literal::CurrentTime,)),
                ],
                ..Default::default()
            }
        );
    }

    #[test]
    fn select_all() {
        let qstring = "SELECT * FROM users;";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: vec![FieldDefinitionExpression::All],
                ..Default::default()
            }
        );
    }

    #[test]
    fn select_all_in_table() {
        let qstring = "SELECT users.* FROM users, votes;";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users"), Table::from("votes")],
                fields: vec![FieldDefinitionExpression::AllInTable(String::from("users"))],
                ..Default::default()
            }
        );
    }

    #[test]
    fn spaces_optional() {
        let qstring = "SELECT id,name FROM users;";

        let res = selection(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: columns(&["id", "name"]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn case_sensitivity() {
        let qstring_lc = "select id, name from users;";
        let qstring_uc = "SELECT id, name FROM users;";

        assert_eq!(
            selection(qstring_lc.as_bytes()).unwrap(),
            selection(qstring_uc.as_bytes()).unwrap()
        );
    }

    #[test]
    fn termination() {
        let qstring_sem = "select id, name from users;";
        let qstring_nosem = "select id, name from users";
        let qstring_linebreak = "select id, name from users\n";

        let r1 = selection(qstring_sem.as_bytes()).unwrap();
        let r2 = selection(qstring_nosem.as_bytes()).unwrap();
        let r3 = selection(qstring_linebreak.as_bytes()).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r2, r3);
    }

    #[test]
    fn where_clause() {
        where_clause_with_variable_placeholder(
            "select * from ContactInfo where email=?;",
            Literal::Placeholder(ItemPlaceholder::QuestionMark),
        );
    }

    #[test]
    fn where_clause_with_dollar_variable() {
        where_clause_with_variable_placeholder(
            "select * from ContactInfo where email= $3;",
            Literal::Placeholder(ItemPlaceholder::DollarNumber(3)),
        );
    }

    #[test]
    fn where_clause_with_colon_variable() {
        where_clause_with_variable_placeholder(
            "select * from ContactInfo where email= :5;",
            Literal::Placeholder(ItemPlaceholder::ColonNumber(5)),
        );
    }

    fn where_clause_with_variable_placeholder(qstring: &str, literal: Literal) {
        let res = selection(qstring.as_bytes());

        let expected_left = Base(Field(Column::from("email")));
        let expected_where_cond = Some(ComparisonOp(ConditionTree {
            left: Box::new(expected_left),
            right: Box::new(Base(Literal(literal))),
            operator: BinaryOperator::Equal,
        }));
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("ContactInfo")],
                fields: vec![FieldDefinitionExpression::All],
                where_clause: expected_where_cond,
                ..Default::default()
            }
        );
    }

    #[test]
    fn limit_clause() {
        let qstring1 = "select * from users limit 10\n";
        let qstring2 = "select * from users limit 10 offset 10\n";

        let expected_lim1 = LimitClause {
            limit: 10,
            offset: 0,
        };
        let expected_lim2 = LimitClause {
            limit: 10,
            offset: 10,
        };

        let res1 = selection(qstring1.as_bytes());
        let res2 = selection(qstring2.as_bytes());
        assert_eq!(res1.unwrap().1.limit, Some(expected_lim1));
        assert_eq!(res2.unwrap().1.limit, Some(expected_lim2));
    }

    #[test]
    fn table_alias() {
        let qstring1 = "select * from PaperTag as t;";
        // let qstring2 = "select * from PaperTag t;";

        let res1 = selection(qstring1.as_bytes());
        assert_eq!(
            res1.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table {
                    name: String::from("PaperTag"),
                    alias: Some(String::from("t")),
                    schema: None,
                },],
                fields: vec![FieldDefinitionExpression::All],
                ..Default::default()
            }
        );
        // let res2 = selection(qstring2.as_bytes());
        // assert_eq!(res1.unwrap().1, res2.unwrap().1);
    }

    #[test]
    fn table_schema() {
        let qstring1 = "select * from db1.PaperTag as t;";

        let res1 = selection(qstring1.as_bytes());
        assert_eq!(
            res1.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table {
                    name: String::from("PaperTag"),
                    alias: Some(String::from("t")),
                    schema: Some(String::from("db1")),
                },],
                fields: vec![FieldDefinitionExpression::All],
                ..Default::default()
            }
        );
        // let res2 = selection(qstring2.as_bytes());
        // assert_eq!(res1.unwrap().1, res2.unwrap().1);
    }

    #[test]
    fn column_alias() {
        let qstring1 = "select name as TagName from PaperTag;";
        let qstring2 = "select PaperTag.name as TagName from PaperTag;";

        let res1 = selection(qstring1.as_bytes());
        assert_eq!(
            res1.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperTag")],
                fields: vec![FieldDefinitionExpression::Expression {
                    alias: Some(String::from("TagName")),
                    expr: Expression::Column(Column::from("name"))
                }],
                ..Default::default()
            }
        );
        let res2 = selection(qstring2.as_bytes());
        assert_eq!(
            res2.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperTag")],
                fields: vec![FieldDefinitionExpression::Expression {
                    expr: Expression::Column(Column::from("PaperTag.name")),
                    alias: Some(String::from("TagName")),
                }],
                ..Default::default()
            }
        );
    }

    #[test]
    fn column_alias_no_as() {
        let qstring1 = "select name TagName from PaperTag;";
        let qstring2 = "select PaperTag.name TagName from PaperTag;";

        let res1 = selection(qstring1.as_bytes());
        assert_eq!(
            res1.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperTag")],
                fields: vec![FieldDefinitionExpression::Expression {
                    alias: Some(String::from("TagName")),
                    expr: Expression::Column(Column {
                        name: String::from("name"),
                        table: None,
                        function: None,
                    })
                }],
                ..Default::default()
            }
        );
        let res2 = selection(qstring2.as_bytes());
        assert_eq!(
            res2.clone().unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperTag")],
                fields: vec![FieldDefinitionExpression::Expression {
                    alias: Some(String::from("TagName")),
                    expr: Expression::Column(Column::from("PaperTag.name"))
                }],
                ..Default::default()
            }
        );
    }

    #[test]
    fn distinct() {
        let qstring = "select distinct tag from PaperTag where paperId=?;";

        let res = selection(qstring.as_bytes());
        let expected_left = Base(Field(Column::from("paperId")));
        let expected_where_cond = Some(ComparisonOp(ConditionTree {
            left: Box::new(expected_left),
            right: Box::new(Base(Literal(Literal::Placeholder(
                ItemPlaceholder::QuestionMark,
            )))),
            operator: BinaryOperator::Equal,
        }));
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperTag")],
                distinct: true,
                fields: columns(&["tag"]),
                where_clause: expected_where_cond,
                ..Default::default()
            }
        );
    }

    #[test]
    fn simple_condition_expr() {
        let qstring = "select infoJson from PaperStorage where paperId=? and paperStorageId=?;";

        let res = selection(qstring.as_bytes());

        let left_ct = ConditionTree {
            left: Box::new(Base(Field(Column::from("paperId")))),
            right: Box::new(Base(Literal(Literal::Placeholder(
                ItemPlaceholder::QuestionMark,
            )))),
            operator: BinaryOperator::Equal,
        };
        let left_comp = Box::new(ComparisonOp(left_ct));
        let right_ct = ConditionTree {
            left: Box::new(Base(Field(Column::from("paperStorageId")))),
            right: Box::new(Base(Literal(Literal::Placeholder(
                ItemPlaceholder::QuestionMark,
            )))),
            operator: BinaryOperator::Equal,
        };
        let right_comp = Box::new(ComparisonOp(right_ct));
        let expected_where_cond = Some(LogicalOp(ConditionTree {
            left: left_comp,
            right: right_comp,
            operator: BinaryOperator::And,
        }));
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("PaperStorage")],
                fields: columns(&["infoJson"]),
                where_clause: expected_where_cond,
                ..Default::default()
            }
        );
    }

    #[test]
    fn where_and_limit_clauses() {
        let qstring = "select * from users where id = ? limit 10\n";
        let res = selection(qstring.as_bytes());

        let expected_lim = Some(LimitClause {
            limit: 10,
            offset: 0,
        });
        let ct = ConditionTree {
            left: Box::new(Base(Field(Column::from("id")))),
            right: Box::new(Base(Literal(Literal::Placeholder(
                ItemPlaceholder::QuestionMark,
            )))),
            operator: BinaryOperator::Equal,
        };
        let expected_where_cond = Some(ComparisonOp(ct));

        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("users")],
                fields: vec![FieldDefinitionExpression::All],
                where_clause: expected_where_cond,
                limit: expected_lim,
                ..Default::default()
            }
        );
    }

    #[test]
    fn aggregation_column() {
        let qstring = "SELECT max(addr_id) FROM address;";

        let res = selection(qstring.as_bytes());
        let agg_expr =
            FunctionExpression::Max(Box::new(Expression::Column(Column::from("addr_id"))));
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("address")],
                fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr),),],
                ..Default::default()
            }
        );
    }

    #[test]
    fn aggregation_column_with_alias() {
        let qstring = "SELECT max(addr_id) AS max_addr FROM address;";

        let res = selection(qstring.as_bytes());
        let agg_expr =
            FunctionExpression::Max(Box::new(Expression::Column(Column::from("addr_id"))));
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("address")],
            fields: vec![FieldDefinitionExpression::Expression {
                alias: Some("max_addr".to_owned()),
                expr: Expression::Call(agg_expr),
            }],
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn count_all() {
        let qstring = "SELECT COUNT(*) FROM votes GROUP BY aid;";

        let res = selection(qstring.as_bytes());
        let agg_expr = FunctionExpression::CountStar;
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("aid")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn count_distinct() {
        let qstring = "SELECT COUNT(DISTINCT vote_id) FROM votes GROUP BY aid;";

        let res = selection(qstring.as_bytes());
        let agg_expr = FunctionExpression::Count {
            expr: Box::new(Expression::Column(Column::from("vote_id"))),
            distinct: true,
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("aid")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn count_filter() {
        let qstring =
            "SELECT COUNT(CASE WHEN vote_id > 10 THEN vote_id END) FROM votes GROUP BY aid;";
        let res = selection(qstring.as_bytes());

        let filter_cond = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("vote_id")))),
            right: Box::new(Base(Literal(Literal::Integer(10.into())))),
            operator: BinaryOperator::Greater,
        });
        let agg_expr = FunctionExpression::Count {
            expr: Box::new(Expression::CaseWhen {
                then_expr: Box::new(Expression::Column(Column::from("vote_id"))),
                else_expr: None,
                condition: filter_cond,
            }),
            distinct: false,
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("aid")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn sum_filter() {
        let qstring = "SELECT SUM(CASE WHEN sign = 1 THEN vote_id END) FROM votes GROUP BY aid;";

        let res = selection(qstring.as_bytes());

        let filter_cond = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("sign")))),
            right: Box::new(Base(Literal(Literal::Integer(1.into())))),
            operator: BinaryOperator::Equal,
        });
        let agg_expr = FunctionExpression::Sum {
            expr: Box::new(Expression::CaseWhen {
                then_expr: Box::new(Expression::Column(Column::from("vote_id"))),
                else_expr: None,
                condition: filter_cond,
            }),
            distinct: false,
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("aid")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn sum_filter_else_literal() {
        let qstring =
            "SELECT SUM(CASE WHEN sign = 1 THEN vote_id ELSE 6 END) FROM votes GROUP BY aid;";

        let res = selection(qstring.as_bytes());

        let filter_cond = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("sign")))),
            right: Box::new(Base(Literal(Literal::Integer(1.into())))),
            operator: BinaryOperator::Equal,
        });
        let agg_expr = FunctionExpression::Sum {
            expr: Box::new(Expression::CaseWhen {
                then_expr: Box::new(Expression::Column(Column::from("vote_id"))),
                else_expr: Some(Box::new(Expression::Literal(Literal::Integer(6)))),
                condition: filter_cond,
            }),
            distinct: false,
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("aid")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn count_filter_lobsters() {
        let qstring = "SELECT
            COUNT(CASE WHEN votes.story_id IS NULL AND votes.vote = 0 THEN votes.vote END) as votes
            FROM votes
            GROUP BY votes.comment_id;";

        let res = selection(qstring.as_bytes());

        let filter_cond = LogicalOp(ConditionTree {
            left: Box::new(ComparisonOp(ConditionTree {
                left: Box::new(Base(Field(Column::from("votes.story_id")))),
                right: Box::new(Base(Literal(Literal::Null))),
                operator: BinaryOperator::Equal,
            })),
            right: Box::new(ComparisonOp(ConditionTree {
                left: Box::new(Base(Field(Column::from("votes.vote")))),
                right: Box::new(Base(Literal(Literal::Integer(0)))),
                operator: BinaryOperator::Equal,
            })),
            operator: BinaryOperator::And,
        });
        let agg_expr = FunctionExpression::Count {
            expr: Box::new(Expression::CaseWhen {
                then_expr: Box::new(Expression::Column(Column::from("votes.vote"))),
                else_expr: None,
                condition: filter_cond,
            }),
            distinct: false,
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("votes")],
            fields: vec![FieldDefinitionExpression::Expression {
                alias: Some("votes".to_owned()),
                expr: Expression::Call(agg_expr),
            }],
            group_by: Some(GroupByClause {
                columns: vec![Column::from("votes.comment_id")],
                having: None,
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn generic_function_query() {
        let qstring = "SELECT coalesce(a, b,c) as x,d FROM sometable;";

        let res = selection(qstring.as_bytes());
        let agg_expr = FunctionExpression::Call {
            name: "coalesce".to_owned(),
            arguments: vec![
                Expression::Column(Column {
                    name: String::from("a"),
                    table: None,
                    function: None,
                }),
                Expression::Column(Column {
                    name: String::from("b"),
                    table: None,
                    function: None,
                }),
                Expression::Column(Column {
                    name: String::from("c"),
                    table: None,
                    function: None,
                }),
            ],
        };
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("sometable")],
            fields: vec![
                FieldDefinitionExpression::Expression {
                    alias: Some("x".to_owned()),
                    expr: Expression::Call(agg_expr),
                },
                FieldDefinitionExpression::from(Column {
                    name: String::from("d"),
                    table: None,
                    function: None,
                }),
            ],
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);
    }

    #[test]
    fn moderately_complex_selection() {
        let qstring = "SELECT * FROM item, author WHERE item.i_a_id = author.a_id AND \
                       item.i_subject = ? ORDER BY item.i_title limit 50;";

        let res = selection(qstring.as_bytes());
        let expected_where_cond = Some(LogicalOp(ConditionTree {
            left: Box::new(ComparisonOp(ConditionTree {
                left: Box::new(Base(Field(Column::from("item.i_a_id")))),
                right: Box::new(Base(Field(Column::from("author.a_id")))),
                operator: BinaryOperator::Equal,
            })),
            right: Box::new(ComparisonOp(ConditionTree {
                left: Box::new(Base(Field(Column::from("item.i_subject")))),
                right: Box::new(Base(Literal(Literal::Placeholder(
                    ItemPlaceholder::QuestionMark,
                )))),
                operator: BinaryOperator::Equal,
            })),
            operator: BinaryOperator::And,
        }));
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("item"), Table::from("author")],
                fields: vec![FieldDefinitionExpression::All],
                where_clause: expected_where_cond,
                order: Some(OrderClause {
                    columns: vec![("item.i_title".into(), OrderType::OrderAscending)],
                }),
                limit: Some(LimitClause {
                    limit: 50,
                    offset: 0,
                }),
                ..Default::default()
            }
        );
    }

    #[test]
    fn simple_joins() {
        let qstring = "select paperId from PaperConflict join PCMember using (contactId);";

        let res = selection(qstring.as_bytes());
        let expected_stmt = SelectStatement {
            tables: vec![Table::from("PaperConflict")],
            fields: columns(&["paperId"]),
            join: vec![JoinClause {
                operator: JoinOperator::Join,
                right: JoinRightSide::Table(Table::from("PCMember")),
                constraint: JoinConstraint::Using(vec![Column::from("contactId")]),
            }],
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected_stmt);

        // slightly simplified from
        // "select PCMember.contactId, group_concat(reviewType separator '')
        // from PCMember left join PaperReview on (PCMember.contactId=PaperReview.contactId)
        // group by PCMember.contactId"
        let qstring = "select PCMember.contactId \
                       from PCMember \
                       join PaperReview on PCMember.contactId=PaperReview.contactId \
                       order by contactId;";

        let res = selection(qstring.as_bytes());
        let ct = ConditionTree {
            left: Box::new(Base(Field(Column::from("PCMember.contactId")))),
            right: Box::new(Base(Field(Column::from("PaperReview.contactId")))),
            operator: BinaryOperator::Equal,
        };
        let join_cond = ConditionExpression::ComparisonOp(ct);
        let expected = SelectStatement {
            tables: vec![Table::from("PCMember")],
            fields: columns(&["PCMember.contactId"]),
            join: vec![JoinClause {
                operator: JoinOperator::Join,
                right: JoinRightSide::Table(Table::from("PaperReview")),
                constraint: JoinConstraint::On(join_cond),
            }],
            order: Some(OrderClause {
                columns: vec![("contactId".into(), OrderType::OrderAscending)],
            }),
            ..Default::default()
        };
        assert_eq!(res.unwrap().1, expected);
    }

    #[test]
    fn multi_join() {
        // simplified from
        // "select max(conflictType), PaperReview.contactId as reviewer, PCMember.contactId as
        //  pcMember, ChairAssistant.contactId as assistant, Chair.contactId as chair,
        //  max(PaperReview.reviewNeedsSubmit) as reviewNeedsSubmit from ContactInfo
        //  left join PaperReview using (contactId) left join PaperConflict using (contactId)
        //  left join PCMember using (contactId) left join ChairAssistant using (contactId)
        //  left join Chair using (contactId) where ContactInfo.contactId=?
        //  group by ContactInfo.contactId;";
        let qstring = "select PCMember.contactId, ChairAssistant.contactId, \
                       Chair.contactId from ContactInfo left join PaperReview using (contactId) \
                       left join PaperConflict using (contactId) left join PCMember using \
                       (contactId) left join ChairAssistant using (contactId) left join Chair \
                       using (contactId) where ContactInfo.contactId=?;";

        let res = selection(qstring.as_bytes());
        let ct = ConditionTree {
            left: Box::new(Base(Field(Column::from("ContactInfo.contactId")))),
            right: Box::new(Base(Literal(Literal::Placeholder(
                ItemPlaceholder::QuestionMark,
            )))),
            operator: BinaryOperator::Equal,
        };
        let expected_where_cond = Some(ComparisonOp(ct));
        let mkjoin = |tbl: &str, col: &str| -> JoinClause {
            JoinClause {
                operator: JoinOperator::LeftJoin,
                right: JoinRightSide::Table(Table::from(tbl)),
                constraint: JoinConstraint::Using(vec![Column::from(col)]),
            }
        };
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec![Table::from("ContactInfo")],
                fields: columns(&[
                    "PCMember.contactId",
                    "ChairAssistant.contactId",
                    "Chair.contactId"
                ]),
                join: vec![
                    mkjoin("PaperReview", "contactId"),
                    mkjoin("PaperConflict", "contactId"),
                    mkjoin("PCMember", "contactId"),
                    mkjoin("ChairAssistant", "contactId"),
                    mkjoin("Chair", "contactId"),
                ],
                where_clause: expected_where_cond,
                ..Default::default()
            }
        );
    }

    #[test]
    fn nested_select() {
        let qstr = "SELECT ol_i_id FROM orders, order_line \
                    WHERE orders.o_c_id IN (SELECT o_c_id FROM orders, order_line \
                    WHERE orders.o_id = order_line.ol_o_id);";

        let res = selection(qstr.as_bytes());
        let inner_where_clause = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("orders.o_id")))),
            right: Box::new(Base(Field(Column::from("order_line.ol_o_id")))),
            operator: BinaryOperator::Equal,
        });

        let inner_select = SelectStatement {
            tables: vec![Table::from("orders"), Table::from("order_line")],
            fields: columns(&["o_c_id"]),
            where_clause: Some(inner_where_clause),
            ..Default::default()
        };

        let outer_where_clause = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("orders.o_c_id")))),
            right: Box::new(Base(NestedSelect(Box::new(inner_select)))),
            operator: BinaryOperator::In,
        });

        let outer_select = SelectStatement {
            tables: vec![Table::from("orders"), Table::from("order_line")],
            fields: columns(&["ol_i_id"]),
            where_clause: Some(outer_where_clause),
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, outer_select);
    }

    #[test]
    fn recursive_nested_select() {
        let qstr = "SELECT ol_i_id FROM orders, order_line WHERE orders.o_c_id \
                    IN (SELECT o_c_id FROM orders, order_line \
                    WHERE orders.o_id = order_line.ol_o_id \
                    AND orders.o_id > (SELECT MAX(o_id) FROM orders));";

        let res = selection(qstr.as_bytes());

        let agg_expr = FunctionExpression::Max(Box::new(Expression::Column(Column::from("o_id"))));
        let recursive_select = SelectStatement {
            tables: vec![Table::from("orders")],
            fields: vec![FieldDefinitionExpression::from(Expression::Call(agg_expr))],
            ..Default::default()
        };

        let cop1 = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("orders.o_id")))),
            right: Box::new(Base(Field(Column::from("order_line.ol_o_id")))),
            operator: BinaryOperator::Equal,
        });

        let cop2 = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("orders.o_id")))),
            right: Box::new(Base(NestedSelect(Box::new(recursive_select)))),
            operator: BinaryOperator::Greater,
        });

        let inner_where_clause = LogicalOp(ConditionTree {
            left: Box::new(cop1),
            right: Box::new(cop2),
            operator: BinaryOperator::And,
        });

        let inner_select = SelectStatement {
            tables: vec![Table::from("orders"), Table::from("order_line")],
            fields: columns(&["o_c_id"]),
            where_clause: Some(inner_where_clause),
            ..Default::default()
        };

        let outer_where_clause = ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("orders.o_c_id")))),
            right: Box::new(Base(NestedSelect(Box::new(inner_select)))),
            operator: BinaryOperator::In,
        });

        let outer_select = SelectStatement {
            tables: vec![Table::from("orders"), Table::from("order_line")],
            fields: columns(&["ol_i_id"]),
            where_clause: Some(outer_where_clause),
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, outer_select);
    }

    #[test]
    fn join_against_nested_select() {
        let t0 = b"(SELECT ol_i_id FROM order_line)";
        let t1 = b"(SELECT ol_i_id FROM order_line) AS ids";

        assert!(join_rhs(t0).is_ok());
        assert!(join_rhs(t1).is_ok());

        let t0 = b"JOIN (SELECT ol_i_id FROM order_line) ON (orders.o_id = ol_i_id)";
        let t1 = b"JOIN (SELECT ol_i_id FROM order_line) AS ids ON (orders.o_id = ids.ol_i_id)";

        assert!(join_clause(t0).is_ok());
        assert!(join_clause(t1).is_ok());

        let qstr_with_alias = "SELECT o_id, ol_i_id FROM orders JOIN \
                               (SELECT ol_i_id FROM order_line) AS ids \
                               ON (orders.o_id = ids.ol_i_id);";
        let res = selection(qstr_with_alias.as_bytes());

        // N.B.: Don't alias the inner select to `inner`, which is, well, a SQL keyword!
        let inner_select = SelectStatement {
            tables: vec![Table::from("order_line")],
            fields: columns(&["ol_i_id"]),
            ..Default::default()
        };

        let outer_select = SelectStatement {
            tables: vec![Table::from("orders")],
            fields: columns(&["o_id", "ol_i_id"]),
            join: vec![JoinClause {
                operator: JoinOperator::Join,
                right: JoinRightSide::NestedSelect(Box::new(inner_select), Some("ids".into())),
                constraint: JoinConstraint::On(Bracketed(Box::new(ComparisonOp(ConditionTree {
                    operator: BinaryOperator::Equal,
                    left: Box::new(Base(Field(Column::from("orders.o_id")))),
                    right: Box::new(Base(Field(Column::from("ids.ol_i_id")))),
                })))),
            }],
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, outer_select);
    }

    #[test]
    fn project_arithmetic_expressions() {
        let qstr = "SELECT MAX(o_id)-3333 FROM orders;";
        let res = selection(qstr.as_bytes());

        let expected = SelectStatement {
            tables: vec![Table::from("orders")],
            fields: vec![FieldDefinitionExpression::from(Expression::Arithmetic(
                Arithmetic {
                    op: ArithmeticOperator::Subtract,
                    left: Box::new(Expression::Call(FunctionExpression::Max(Box::new(
                        Expression::Column("o_id".into()),
                    )))),
                    right: Box::new(Expression::Literal(3333.into())),
                },
            ))],
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, expected);
    }

    #[test]
    fn project_arithmetic_expressions_with_aliases() {
        let qstr = "SELECT max(o_id) * 2 as double_max FROM orders;";
        let res = selection(qstr.as_bytes());

        let expected = SelectStatement {
            tables: vec![Table::from("orders")],
            fields: vec![FieldDefinitionExpression::Expression {
                alias: Some(String::from("double_max")),
                expr: Expression::Arithmetic(Arithmetic {
                    op: ArithmeticOperator::Multiply,
                    left: Box::new(Expression::Call(FunctionExpression::Max(Box::new(
                        Expression::Column("o_id".into()),
                    )))),
                    right: Box::new(Expression::Literal(2.into())),
                }),
            }],
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, expected);
    }

    #[test]
    fn where_in_clause() {
        let qstr = "SELECT `auth_permission`.`content_type_id`, `auth_permission`.`codename`
                    FROM `auth_permission`
                    JOIN `django_content_type`
                      ON ( `auth_permission`.`content_type_id` = `django_content_type`.`id` )
                    WHERE `auth_permission`.`content_type_id` IN (0);";
        let res = selection(qstr.as_bytes());

        let expected_where_clause = Some(ComparisonOp(ConditionTree {
            left: Box::new(Base(Field(Column::from("auth_permission.content_type_id")))),
            right: Box::new(Base(LiteralList(vec![0.into()]))),
            operator: BinaryOperator::In,
        }));

        let expected = SelectStatement {
            tables: vec![Table::from("auth_permission")],
            fields: vec![
                FieldDefinitionExpression::from(Column::from("auth_permission.content_type_id")),
                FieldDefinitionExpression::from(Column::from("auth_permission.codename")),
            ],
            join: vec![JoinClause {
                operator: JoinOperator::Join,
                right: JoinRightSide::Table(Table::from("django_content_type")),
                constraint: JoinConstraint::On(Bracketed(Box::new(ComparisonOp(ConditionTree {
                    operator: BinaryOperator::Equal,
                    left: Box::new(Base(Field(Column::from("auth_permission.content_type_id")))),
                    right: Box::new(Base(Field(Column::from("django_content_type.id")))),
                })))),
            }],
            where_clause: expected_where_clause,
            ..Default::default()
        };

        assert_eq!(res.unwrap().1, expected);
    }

    #[test]
    fn alias_cast() {
        let qstr = "SELECT id, CAST(created_at AS date) AS created_day FROM users WHERE id = ?;";
        let res = selection(qstr.as_bytes());
        assert!(res.is_ok(), "!{:?}.is_ok()", res);
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec!["users".into()],
                fields: vec![
                    FieldDefinitionExpression::from(Column::from("id")),
                    FieldDefinitionExpression::Expression {
                        alias: Some("created_day".to_owned()),
                        expr: Expression::Call(FunctionExpression::Cast(
                            Box::new(Expression::Column(Column::from("created_at"))),
                            SqlType::Date
                        )),
                    },
                ],
                where_clause: Some(ComparisonOp(ConditionTree {
                    left: Box::new(Base(Field(Column::from("id")))),
                    right: Box::new(Base(Literal(Literal::Placeholder(
                        ItemPlaceholder::QuestionMark
                    )))),
                    operator: BinaryOperator::Equal,
                })),
                ..Default::default()
            }
        )
    }

    #[test]
    fn alias_generic_function() {
        let qstr = "SELECT id, coalesce(a, \"b\",c) AS created_day FROM users;";
        let res = selection(qstr.as_bytes());
        assert!(res.is_ok(), "!{:?}.is_ok()", res);
        assert_eq!(
            res.unwrap().1,
            SelectStatement {
                tables: vec!["users".into()],
                fields: vec![
                    FieldDefinitionExpression::from(Column::from("id")),
                    FieldDefinitionExpression::Expression {
                        alias: Some("created_day".to_owned()),
                        expr: Expression::Call(FunctionExpression::Call {
                            name: "coalesce".to_owned(),
                            arguments: vec![
                                Expression::Column(Column::from("a")),
                                Expression::Literal(Literal::String("b".to_owned())),
                                Expression::Column(Column::from("c"))
                            ]
                        }),
                    },
                ],
                where_clause: None,
                ..Default::default()
            }
        )
    }
    #[test]
    fn simple_cte() {
        let qstr = b"WITH max_val AS (SELECT max(value) as value FROM t1)
            SELECT name FROM t2 JOIN max_val ON max_val.value = t2.value";
        let res = selection(qstr);
        assert!(res.is_ok(), "error parsing query: {}", res.err().unwrap());
        let (rem, query) = res.unwrap();
        assert!(rem.is_empty());
        assert_eq!(query.ctes.len(), 1);
        assert_eq!(query.ctes.first().unwrap().name, "max_val".to_owned());
    }

    #[test]
    fn multiple_ctes() {
        let qstr = b"WITH
              max_val AS (SELECT max(value) as value FROM t1),
              min_val AS (SELECT min(value) as value FROM t1)
            SELECT name FROM t2
            JOIN max_val ON max_val.value = t2.max_value
            JOIN min_val ON min_val.value = t2.min_value";
        let res = selection(qstr);
        assert!(res.is_ok(), "error parsing query: {}", res.err().unwrap());
        let (rem, query) = res.unwrap();
        assert!(rem.is_empty());
        assert_eq!(query.ctes.len(), 2);
        assert_eq!(query.ctes[0].name, "max_val".to_owned());
        assert_eq!(query.ctes[1].name, "min_val".to_owned());
    }
}
