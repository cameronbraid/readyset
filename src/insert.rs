use nom::multispace;
use std::str;
use std::fmt;

use common::{field_list, opt_multispace, statement_terminator, table_reference, value_list,
             Literal};
use column::Column;
use table::Table;

#[derive(Clone, Debug, Default, Hash, PartialEq, Serialize, Deserialize)]
pub struct InsertStatement {
    pub table: Table,
    pub fields: Vec<(Column, Literal)>,
    pub ignore: bool,
}

impl fmt::Display for InsertStatement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "INSERT INTO {}", self.table)?;
        write!(
            f,
            " ({})",
            self.fields
                .iter()
                .map(|&(ref col, _)| col.name.to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        write!(
            f,
            " VALUES ({})",
            self.fields
                .iter()
                .map(|&(_, ref literal)| literal.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Parse rule for a SQL insert query.
/// TODO(malte): support REPLACE, multiple parens expr, nested selection, DEFAULT VALUES
named!(pub insertion<&[u8], InsertStatement>,
    complete!(do_parse!(
        tag_no_case!("insert") >>
        ignore: opt!(preceded!(multispace, tag_no_case!("ignore"))) >>
        multispace >>
        tag_no_case!("into") >>
        multispace >>
        table: table_reference >>
        opt_multispace >>
        fields: opt!(do_parse!(
                tag!("(") >>
                opt_multispace >>
                fields: field_list >>
                opt_multispace >>
                tag!(")") >>
                multispace >>
                (fields)
                )
            ) >>
        tag_no_case!("values") >>
        opt_multispace >>
        tag!("(") >>
        values: value_list >>
        tag!(")") >>
        statement_terminator >>
        ({
            // "table AS alias" isn't legal in INSERT statements
            assert!(table.alias.is_none());
            InsertStatement {
                table: table,
                fields: match fields {
                    Some(ref f) =>
                        f.iter()
                         .cloned()
                         .zip(values.into_iter())
                         .collect(),
                    None =>
                        values.into_iter()
                              .enumerate()
                              .map(|(i, v)| {
                                  (Column::from(format!("{}", i).as_str()), v)
                              })
                              .collect(),
                },
                ignore: ignore.is_some(),
            }
        })
    ))
);

#[cfg(test)]
mod tests {
    use super::*;
    use column::Column;
    use table::Table;

    #[test]
    fn simple_insert() {
        let qstring = "INSERT INTO users VALUES (42, \"test\");";

        let res = insertion(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            InsertStatement {
                table: Table::from("users"),
                fields: vec![
                    (Column::from("0"), 42.into()),
                    (Column::from("1"), "test".into()),
                ],
                ..Default::default()
            }
        );
    }

    #[test]
    fn complex_insert() {
        let qstring = "INSERT INTO users VALUES (42, 'test', \"test\", CURRENT_TIMESTAMP);";

        let res = insertion(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            InsertStatement {
                table: Table::from("users"),
                fields: vec![
                    (Column::from("0"), 42.into()),
                    (Column::from("1"), "test".into()),
                    (Column::from("2"), "test".into()),
                    (Column::from("3"), Literal::CurrentTimestamp),
                ],
                ..Default::default()
            }
        );
    }

    #[test]
    fn insert_with_field_names() {
        let qstring = "INSERT INTO users (id, name) VALUES (42, \"test\");";

        let res = insertion(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            InsertStatement {
                table: Table::from("users"),
                fields: vec![
                    (Column::from("id"), 42.into()),
                    (Column::from("name"), "test".into()),
                ],
                ..Default::default()
            }
        );
    }

    // Issue #3
    #[test]
    fn insert_without_spaces() {
        let qstring = "INSERT INTO users(id, name) VALUES(42, \"test\");";

        let res = insertion(qstring.as_bytes());
        assert_eq!(
            res.unwrap().1,
            InsertStatement {
                table: Table::from("users"),
                fields: vec![
                    (Column::from("id"), 42.into()),
                    (Column::from("name"), "test".into()),
                ],
                ..Default::default()
            }
        );
    }
}
