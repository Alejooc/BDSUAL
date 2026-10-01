//! Validación acotada de CSV y celdas antes de preparar una revisión de importación.

use serde::{Deserialize, Serialize};
use std::io::Read;

const MAX_CSV_BYTES: u64 = 5 * 1024 * 1024;
const MAX_CSV_ROWS: usize = 50_000;
const MAX_CSV_COLUMNS: usize = 512;
/// Tope del plan serializado que puede convertirse en un artefacto cifrado.
pub(crate) const MAX_IMPORT_PLAN_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_IMPORT_ROWS: usize = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParsedCsv {
    pub headers: Vec<String>,
    /// `None` representa únicamente el marcador NULL configurado.
    pub rows: Vec<Vec<Option<String>>>,
}

/// Resumen serializable seguro para la vista de revisión; no incluye el archivo original.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CsvBatchPreview {
    pub headers: Vec<String>,
    pub row_count: usize,
    pub sample_rows: Vec<Vec<Option<String>>>,
}

pub(crate) fn preview(headers: &[String], rows: &[Vec<Option<String>>]) -> CsvBatchPreview {
    CsvBatchPreview {
        headers: headers.to_vec(),
        row_count: rows.len(),
        sample_rows: rows.iter().take(5).cloned().collect(),
    }
}

/// Rechaza claves ausentes, NULL y duplicadas dentro del archivo antes de consultar el destino.
/// Las claves se comparan como celdas CSV decodificadas, preservando NULL/texto y orden compuesto.
pub(crate) fn validate_unique_primary_keys(
    parsed: &ParsedCsv,
    primary_key_columns: &[String],
) -> Result<(), CsvImportError> {
    if parsed.rows.is_empty()
        || parsed.rows.len() > MAX_IMPORT_ROWS
        || primary_key_columns.is_empty()
    {
        return Err(CsvImportError::InvalidCsv);
    }
    let indexes = primary_key_columns
        .iter()
        .map(|column| {
            parsed
                .headers
                .iter()
                .position(|header| header == column)
                .ok_or(CsvImportError::InvalidHeaders)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut keys = std::collections::HashSet::with_capacity(parsed.rows.len());
    for row in &parsed.rows {
        let key = indexes
            .iter()
            .map(|index| row.get(*index).cloned().ok_or(CsvImportError::InvalidCsv))
            .collect::<Result<Vec<_>, _>>()?;
        if key.iter().any(Option::is_none) || !keys.insert(key) {
            return Err(CsvImportError::InvalidHeaders);
        }
    }
    Ok(())
}

pub(crate) fn enforce_plan_size<T: Serialize>(plan: &T) -> Result<Vec<u8>, CsvImportError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| CsvImportError::InvalidCsv)?;
    if bytes.len() > MAX_IMPORT_PLAN_BYTES {
        return Err(CsvImportError::TooLarge);
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CsvDelimiter {
    Comma,
    Semicolon,
    Tab,
}

impl CsvDelimiter {
    fn byte(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CsvImportError {
    TooLarge,
    InvalidUtf8,
    InvalidCsv,
    InvalidHeaders,
    TooManyRows,
}

pub(crate) fn parse<R: Read>(
    mut input: R,
    delimiter: CsvDelimiter,
    null_marker: &str,
) -> Result<ParsedCsv, CsvImportError> {
    if null_marker.len() > 128 || null_marker.contains(['\r', '\n']) {
        return Err(CsvImportError::InvalidCsv);
    }
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(MAX_CSV_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CsvImportError::InvalidCsv)?;
    if bytes.len() as u64 > MAX_CSV_BYTES {
        return Err(CsvImportError::TooLarge);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| CsvImportError::InvalidUtf8)?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter.byte())
        .has_headers(true)
        .flexible(false)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|_| CsvImportError::InvalidCsv)?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if headers.is_empty()
        || headers.len() > MAX_CSV_COLUMNS
        || headers
            .iter()
            .any(|name| name.is_empty() || name.trim() != name)
    {
        return Err(CsvImportError::InvalidHeaders);
    }
    let mut unique = std::collections::HashSet::with_capacity(headers.len());
    if headers
        .iter()
        .any(|header| !unique.insert(header.to_lowercase()))
    {
        return Err(CsvImportError::InvalidHeaders);
    }

    let mut rows = Vec::new();
    for record in reader.records() {
        if rows.len() == MAX_CSV_ROWS {
            return Err(CsvImportError::TooManyRows);
        }
        let record = record.map_err(|_| CsvImportError::InvalidCsv)?;
        if record.len() != headers.len() {
            return Err(CsvImportError::InvalidCsv);
        }
        rows.push(
            record
                .iter()
                .map(|value| decode_cell(value, null_marker))
                .collect(),
        );
    }
    if rows.is_empty() {
        return Err(CsvImportError::InvalidCsv);
    }
    Ok(ParsedCsv { headers, rows })
}

/// Alineado con `sql_editor::escape_csv_cell`: una barra inicial adicional
/// protege tanto el marcador NULL literal como los valores que ya empiezan por `\\`.
fn decode_cell(value: &str, null_marker: &str) -> Option<String> {
    if value == null_marker {
        return None;
    }
    if value.starts_with("\\\\")
        || (!null_marker.is_empty() && value.starts_with('\\') && &value[1..] == null_marker)
    {
        return Some(value[1..].to_owned());
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_csv_and_round_trips_null_marker_escaping() {
        let csv = "id,text,note\n1,\"hola, mundo\",\\N\n2,\\\\N,\"linea\nuno\"\n";
        let parsed = parse(csv.as_bytes(), CsvDelimiter::Comma, "\\N").unwrap();
        assert_eq!(parsed.headers, ["id", "text", "note"]);
        assert_eq!(
            parsed.rows[0],
            [Some("1".into()), Some("hola, mundo".into()), None]
        );
        assert_eq!(
            parsed.rows[1],
            [
                Some("2".into()),
                Some("\\N".into()),
                Some("linea\nuno".into())
            ]
        );
    }

    #[test]
    fn rejects_duplicate_headers_and_ragged_rows() {
        assert_eq!(
            parse("Name,name\na,b\n".as_bytes(), CsvDelimiter::Comma, "NULL"),
            Err(CsvImportError::InvalidHeaders)
        );
        assert_eq!(
            parse("a,b\n1\n".as_bytes(), CsvDelimiter::Comma, "NULL"),
            Err(CsvImportError::InvalidCsv)
        );
    }

    #[test]
    fn parses_alternate_separator_and_requires_nonempty_rows() {
        let parsed = parse(
            "id;name\n1;Ana\n".as_bytes(),
            CsvDelimiter::Semicolon,
            "NULL",
        )
        .unwrap();
        assert_eq!(
            parsed.rows,
            vec![vec![Some("1".into()), Some("Ana".into())]]
        );
        assert_eq!(
            parse("id,name\n".as_bytes(), CsvDelimiter::Comma, "NULL"),
            Err(CsvImportError::InvalidCsv)
        );
    }

    #[test]
    fn batch_preview_is_sampled_and_duplicate_or_null_keys_are_rejected() {
        let parsed = parse(
            "id,name\n1,Ana\n2,Bea\n".as_bytes(),
            CsvDelimiter::Comma,
            "NULL",
        )
        .unwrap();
        let preview = preview(&parsed.headers, &parsed.rows);
        assert_eq!(preview.row_count, 2);
        assert_eq!(preview.sample_rows.len(), 2);
        assert_eq!(
            validate_unique_primary_keys(&parsed, &["id".into()]),
            Ok(())
        );

        let duplicate = parse(
            "id,name\n1,Ana\n1,Bea\n".as_bytes(),
            CsvDelimiter::Comma,
            "NULL",
        )
        .unwrap();
        assert_eq!(
            validate_unique_primary_keys(&duplicate, &["id".into()]),
            Err(CsvImportError::InvalidHeaders)
        );
        let null_key = parse(
            "id,name\nNULL,Ana\n".as_bytes(),
            CsvDelimiter::Comma,
            "NULL",
        )
        .unwrap();
        assert_eq!(
            validate_unique_primary_keys(&null_key, &["id".into()]),
            Err(CsvImportError::InvalidHeaders)
        );
    }

    #[test]
    fn import_plan_size_is_capped() {
        #[derive(Serialize)]
        struct Plan(String);
        let small = Plan("ok".into());
        assert!(enforce_plan_size(&small).is_ok());
        let large = Plan("x".repeat(MAX_IMPORT_PLAN_BYTES));
        assert_eq!(enforce_plan_size(&large), Err(CsvImportError::TooLarge));
    }
}
