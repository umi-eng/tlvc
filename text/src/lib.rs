// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use serde::{de::Error, Deserialize, Serialize};
use std::io;
use zerocopy::AsBytes;

pub fn load(input: impl io::Read) -> io::Result<Vec<Piece>> {
    ron::de::from_reader(input)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

pub fn from_str(input: &str) -> ron::error::SpannedResult<Vec<Piece>> {
    ron::de::from_str(input)
}

pub fn save(output: impl io::Write, pieces: &[Piece]) -> io::Result<()> {
    ron::ser::to_writer_pretty(
        output,
        pieces,
        ron::ser::PrettyConfig::default(),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(untagged)]
pub enum Piece {
    Chunk(Tag, Vec<Piece>),
    Bytes(Vec<u8>),
    String(String),
}

impl Piece {
    pub fn append_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::Bytes(bytes) => out.extend(bytes),
            Self::Chunk(tag, children) => {
                let mut header = tlvc::ChunkHeader {
                    tag: (*tag).into(),
                    len: 0.into(),
                    header_checksum: 0.into(),
                };
                let header_pos = out.len();
                out.extend(header.as_bytes());

                let body_pos = out.len();
                for child in children {
                    child.append_to(out);
                }
                let body_crc = tlvc::compute_body_crc(&out[body_pos..]);

                let body_len = u32::try_from(out.len() - body_pos).unwrap();
                while out.len() & 0b11 != 0 {
                    out.push(0);
                }
                out.extend(body_crc.to_le_bytes());

                header.len.set(body_len);
                header.header_checksum.set(header.compute_checksum());

                out[header_pos
                    ..(header_pos + std::mem::size_of::<tlvc::ChunkHeader>())]
                    .copy_from_slice(header.as_bytes());
            }
            Self::String(str) => out.extend(str.as_bytes()),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Tag([u8; 4]);

impl Tag {
    pub fn new(bytes: [u8; 4]) -> Self {
        // Guard against non-UTF8 data.
        if std::str::from_utf8(&bytes).is_err() {
            panic!("invalid tag: {:x?}", bytes);
        }

        Self(bytes)
    }
}

impl From<Tag> for [u8; 4] {
    fn from(t: Tag) -> Self {
        t.0
    }
}

impl Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        let s = std::str::from_utf8(&self.0).unwrap();
        s.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        let s: &str = Deserialize::deserialize(deserializer)?;
        let s: &[u8; 4] = s
            .as_bytes()
            .try_into()
            .map_err(|_| D::Error::custom("wrong tag length"))?;
        Ok(Tag(*s))
    }
}

pub fn pack<'a>(pieces: impl IntoIterator<Item = &'a Piece>) -> Vec<u8> {
    let mut out = vec![];

    for piece in pieces {
        piece.append_to(&mut out);
    }

    out
}

#[test]
fn pack_unpack() {
    use tlvc::TlvcReader;
    let t = load(
        r#"
            [
                ("BARC", [
                    ("FOOB", [[8, 6, 7, 5, 3, 0, 9]]),
                    ("QUUX", []),
                ]),
            ]
        "#
        .as_bytes(),
    )
    .unwrap();
    assert!(t.len() == 1);
    assert_eq!(
        t[0],
        Piece::Chunk(
            Tag(*b"BARC"),
            vec![
                Piece::Chunk(
                    Tag(*b"FOOB"),
                    vec![Piece::Bytes(vec![8, 6, 7, 5, 3, 0, 9])]
                ),
                Piece::Chunk(Tag(*b"QUUX"), vec![])
            ]
        )
    );
    let data: Vec<u8> = pack(&t);
    let mut reader = TlvcReader::begin(data.as_slice()).unwrap();
    let mut buf = [0u8; 256];

    let barc_chunk = reader.next().unwrap().unwrap();
    barc_chunk.check_body_checksum(&mut buf).unwrap();
    assert_eq!(barc_chunk.header().tag, *b"BARC");

    let mut barc_reader = barc_chunk.read_as_chunks();
    let foob_chunk = barc_reader.next().unwrap().unwrap();
    foob_chunk.check_body_checksum(&mut buf).unwrap();
    assert_eq!(foob_chunk.len(), 7);
    assert!(!foob_chunk.is_empty());
    let mut foob_data = [0u8; 7];
    foob_chunk.read_exact(0, &mut foob_data).unwrap();
    assert_eq!(foob_data, [8, 6, 7, 5, 3, 0, 9]);
    assert_eq!(foob_chunk.header().tag, *b"FOOB");

    let quux_chunk = barc_reader.next().unwrap().unwrap();
    quux_chunk.check_body_checksum(&mut buf).unwrap();
    assert_eq!(quux_chunk.len(), 0);
    assert!(quux_chunk.is_empty());
    assert_eq!(quux_chunk.header().tag, *b"QUUX");

    assert!(barc_reader.next().unwrap().is_none());
    assert!(reader.next().unwrap().is_none());
}
