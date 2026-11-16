//! Just enough of CRI's @UTF table format to read an ACB's cue sheet name
//! (header table, row 0, column `Name`). Pure and tested.

fn be16(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}
fn be32(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// Size in bytes of a value of column type `ty` (low nibble of the flags).
fn type_size(ty: u8) -> Option<usize> {
    Some(match ty {
        0 | 1 => 1,
        2 | 3 => 2,
        4 | 5 | 8 | 0xA => 4,
        6 | 7 | 9 | 0xB => 8,
        _ => return None,
    })
}

fn string_at(t: &[u8], str_off: usize, o: usize) -> Option<String> {
    let start = str_off + o;
    let rest = t.get(start..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Row-0 string column `column` of the @UTF table at the start of `b`.
pub fn row0_string(b: &[u8], column: &str) -> Option<String> {
    if b.get(0..4)? != b"@UTF" {
        return None;
    }
    let size = be32(b, 4)?;
    let t = b.get(8..8 + size.min(b.len().saturating_sub(8)))?;
    let rows_off = be16(t, 2)?;
    let str_off = be32(t, 4)?;
    let ncol = be16(t, 16)?;
    let mut p = 24;
    let mut row_pos = rows_off;
    for _ in 0..ncol {
        let flags = *t.get(p)?;
        let name = string_at(t, str_off, be32(t, p + 1)?)?;
        p += 5;
        let storage = flags & 0xF0;
        let ty = flags & 0x0F;
        let n = type_size(ty)?;
        let wanted = name == column && ty == 0xA;
        match storage {
            0x30 | 0x70 => {
                if wanted {
                    return string_at(t, str_off, be32(t, p)?);
                }
                p += n;
            }
            0x50 => {
                if wanted {
                    return string_at(t, str_off, be32(t, row_pos)?);
                }
                row_pos += n;
            }
            _ => {
                if wanted {
                    return None;
                }
            }
        }
    }
    None
}

/// The cue sheet name inside an ACB file's bytes (e.g. `snd_voi_race_100100`).
pub fn acb_name(b: &[u8]) -> Option<String> {
    row0_string(b, "Name").filter(|s| !s.is_empty())
}

#[cfg(test)]
pub mod tests {
    /// Builds a one-row @UTF table: (column, per-row?, string value) + an int column.
    pub fn table(cols: &[(&str, bool, &str)]) -> Vec<u8> {
        let mut strings = b"<NULL>\0Tbl\0".to_vec();
        let mut add = |s: &str| {
            let o = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            o
        };
        let mut schema = Vec::new();
        let mut ncol: u16 = 1;
        let mut row = Vec::new();
        // A leading per-row u16 column to make offsets non-trivial.
        schema.push(0x52);
        schema.extend_from_slice(&add("Version").to_be_bytes());
        row.extend_from_slice(&7u16.to_be_bytes());
        for (name, per_row, value) in cols {
            let n = add(name);
            let v = add(value);
            if *per_row {
                ncol += 1;
                schema.push(0x5A);
                schema.extend_from_slice(&n.to_be_bytes());
                row.extend_from_slice(&v.to_be_bytes());
            } else {
                ncol += 2;
                schema.push(0x1A); // zero storage first, then a constant
                schema.extend_from_slice(&add("Zero").to_be_bytes());
                schema.push(0x3A);
                schema.extend_from_slice(&n.to_be_bytes());
                schema.extend_from_slice(&v.to_be_bytes());
            }
        }
        let rows_off = 24 + schema.len();
        let str_off = rows_off + row.len();
        let data_off = str_off + strings.len();
        let mut t = Vec::new();
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&(rows_off as u16).to_be_bytes());
        t.extend_from_slice(&(str_off as u32).to_be_bytes());
        t.extend_from_slice(&(data_off as u32).to_be_bytes());
        t.extend_from_slice(&7u32.to_be_bytes()); // table name: "Tbl"
        t.extend_from_slice(&ncol.to_be_bytes());
        t.extend_from_slice(&(row.len() as u16).to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&schema);
        t.extend_from_slice(&row);
        t.extend_from_slice(&strings);
        let mut b = b"@UTF".to_vec();
        b.extend_from_slice(&(t.len() as u32).to_be_bytes());
        b.extend_from_slice(&t);
        b.extend_from_slice(&[0u8; 32]); // whatever follows the table
        b
    }

    #[test]
    fn reads_per_row_name() {
        let b = table(&[("Other", true, "x"), ("Name", true, "snd_voi_race_100100")]);
        assert_eq!(super::acb_name(&b).as_deref(), Some("snd_voi_race_100100"));
    }

    #[test]
    fn reads_constant_name() {
        let b = table(&[("Name", false, "snd_bgm_AM003")]);
        assert_eq!(super::acb_name(&b).as_deref(), Some("snd_bgm_AM003"));
    }

    #[test]
    fn rejects_other_data() {
        assert_eq!(super::acb_name(b"AFS2...."), None);
        assert_eq!(super::acb_name(b"@UTF"), None);
        let mut b = table(&[("Name", true, "abc")]);
        b.truncate(20);
        assert_eq!(super::acb_name(&b), None);
    }
}
