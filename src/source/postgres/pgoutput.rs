use crate::error::{MeiliBridgeError, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use tracing::debug;

#[derive(Debug, Clone)]
pub enum PgOutputMessage {
    Begin(BeginMessage),
    Commit(CommitMessage),
    Origin(OriginMessage),
    Relation(RelationMessage),
    Type(TypeMessage),
    Insert(InsertMessage),
    Update(UpdateMessage),
    Delete(DeleteMessage),
    Truncate(TruncateMessage),
    // Keepalive and other control messages
    Keepalive(KeepaliveMessage),
}

#[derive(Debug, Clone)]
pub struct BeginMessage {
    pub final_lsn: u64,
    pub timestamp: i64,
    pub xid: u32,
}

#[derive(Debug, Clone)]
pub struct CommitMessage {
    pub flags: u8,
    pub commit_lsn: u64,
    pub end_lsn: u64,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct OriginMessage {
    pub commit_lsn: u64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RelationMessage {
    pub relation_id: u32,
    pub namespace: String,
    pub relation_name: String,
    pub replica_identity: u8,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub flags: u8,
    pub name: String,
    pub type_id: u32,
    pub type_modifier: i32,
}

#[derive(Debug, Clone)]
pub struct TypeMessage {
    pub type_id: u32,
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct InsertMessage {
    pub relation_id: u32,
    pub tuple: TupleData,
}

#[derive(Debug, Clone)]
pub struct UpdateMessage {
    pub relation_id: u32,
    pub old_tuple: Option<TupleData>,
    pub new_tuple: TupleData,
}

#[derive(Debug, Clone)]
pub struct DeleteMessage {
    pub relation_id: u32,
    pub old_tuple: TupleData,
}

#[derive(Debug, Clone)]
pub struct TruncateMessage {
    pub relation_count: u32,
    pub options: u8,
    pub relation_ids: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct TupleData {
    pub columns: Vec<Option<Vec<u8>>>,
}

#[derive(Debug, Clone)]
pub struct KeepaliveMessage {
    pub wal_end: u64,
    pub timestamp: i64,
    pub reply_requested: bool,
}

#[derive(Debug, Clone)]
pub struct PgOutputParser {
    relations: HashMap<u32, RelationMessage>,
}

impl PgOutputParser {
    pub fn new() -> Self {
        Self {
            relations: HashMap::new(),
        }
    }

    pub fn parse_message(&mut self, data: &[u8]) -> Result<Option<PgOutputMessage>> {
        if data.is_empty() {
            return Ok(None);
        }

        let msg_type = data[0];
        let payload = &data[1..];

        match msg_type {
            b'B' => Ok(Some(PgOutputMessage::Begin(self.parse_begin(payload)?))),
            b'C' => Ok(Some(PgOutputMessage::Commit(self.parse_commit(payload)?))),
            b'O' => Ok(Some(PgOutputMessage::Origin(self.parse_origin(payload)?))),
            b'R' => {
                let relation = self.parse_relation(payload)?;
                self.relations
                    .insert(relation.relation_id, relation.clone());
                Ok(Some(PgOutputMessage::Relation(relation)))
            }
            b'Y' => Ok(Some(PgOutputMessage::Type(self.parse_type(payload)?))),
            b'I' => Ok(Some(PgOutputMessage::Insert(self.parse_insert(payload)?))),
            b'U' => Ok(Some(PgOutputMessage::Update(self.parse_update(payload)?))),
            b'D' => Ok(Some(PgOutputMessage::Delete(self.parse_delete(payload)?))),
            b'T' => Ok(Some(PgOutputMessage::Truncate(
                self.parse_truncate(payload)?,
            ))),
            b'k' => Ok(Some(PgOutputMessage::Keepalive(
                self.parse_keepalive(payload)?,
            ))),
            _ => {
                debug!("Unknown pgoutput message type: {}", msg_type as char);
                Ok(None)
            }
        }
    }

    fn parse_begin(&self, data: &[u8]) -> Result<BeginMessage> {
        let mut cursor = Cursor::new(data);
        let final_lsn = cursor.read_u64::<BigEndian>()?;
        let timestamp = cursor.read_i64::<BigEndian>()?;
        let xid = cursor.read_u32::<BigEndian>()?;

        Ok(BeginMessage {
            final_lsn,
            timestamp,
            xid,
        })
    }

    fn parse_commit(&self, data: &[u8]) -> Result<CommitMessage> {
        let mut cursor = Cursor::new(data);
        let flags = cursor.read_u8()?;
        let commit_lsn = cursor.read_u64::<BigEndian>()?;
        let end_lsn = cursor.read_u64::<BigEndian>()?;
        let timestamp = cursor.read_i64::<BigEndian>()?;

        Ok(CommitMessage {
            flags,
            commit_lsn,
            end_lsn,
            timestamp,
        })
    }

    fn parse_origin(&self, data: &[u8]) -> Result<OriginMessage> {
        let mut cursor = Cursor::new(data);
        let commit_lsn = cursor.read_u64::<BigEndian>()?;
        let name = self.read_string(&mut cursor)?;

        Ok(OriginMessage { commit_lsn, name })
    }

    fn parse_relation(&self, data: &[u8]) -> Result<RelationMessage> {
        let mut cursor = Cursor::new(data);
        let relation_id = cursor.read_u32::<BigEndian>()?;
        let namespace = self.read_string(&mut cursor)?;
        let relation_name = self.read_string(&mut cursor)?;
        let replica_identity = cursor.read_u8()?;
        let column_count = cursor.read_u16::<BigEndian>()?;

        let mut columns = Vec::with_capacity(column_count as usize);
        for _ in 0..column_count {
            let flags = cursor.read_u8()?;
            let name = self.read_string(&mut cursor)?;
            let type_id = cursor.read_u32::<BigEndian>()?;
            let type_modifier = cursor.read_i32::<BigEndian>()?;

            columns.push(Column {
                flags,
                name,
                type_id,
                type_modifier,
            });
        }

        Ok(RelationMessage {
            relation_id,
            namespace,
            relation_name,
            replica_identity,
            columns,
        })
    }

    fn parse_type(&self, data: &[u8]) -> Result<TypeMessage> {
        let mut cursor = Cursor::new(data);
        let type_id = cursor.read_u32::<BigEndian>()?;
        let namespace = self.read_string(&mut cursor)?;
        let name = self.read_string(&mut cursor)?;

        Ok(TypeMessage {
            type_id,
            namespace,
            name,
        })
    }

    fn parse_insert(&self, data: &[u8]) -> Result<InsertMessage> {
        let mut cursor = Cursor::new(data);
        let relation_id = cursor.read_u32::<BigEndian>()?;
        let tuple_type = cursor.read_u8()?;

        if tuple_type != b'N' {
            return Err(MeiliBridgeError::Source(format!(
                "Expected 'N' tuple type for INSERT, got {}",
                tuple_type as char
            )));
        }

        let tuple = self.parse_tuple_data(&mut cursor)?;

        Ok(InsertMessage { relation_id, tuple })
    }

    fn parse_update(&self, data: &[u8]) -> Result<UpdateMessage> {
        let mut cursor = Cursor::new(data);
        let relation_id = cursor.read_u32::<BigEndian>()?;

        let mut old_tuple = None;
        let mut new_tuple = None;

        // Read tuple type
        while cursor.position() < data.len() as u64 {
            let tuple_type = cursor.read_u8()?;
            match tuple_type {
                b'O' | b'K' => {
                    // Old tuple (O = old, K = old key)
                    old_tuple = Some(self.parse_tuple_data(&mut cursor)?);
                }
                b'N' => {
                    // New tuple
                    new_tuple = Some(self.parse_tuple_data(&mut cursor)?);
                }
                _ => {
                    return Err(MeiliBridgeError::Source(format!(
                        "Unknown tuple type in UPDATE: {}",
                        tuple_type as char
                    )));
                }
            }
        }

        let new_tuple = new_tuple
            .ok_or_else(|| MeiliBridgeError::Source("UPDATE missing new tuple".to_string()))?;

        Ok(UpdateMessage {
            relation_id,
            old_tuple,
            new_tuple,
        })
    }

    fn parse_delete(&self, data: &[u8]) -> Result<DeleteMessage> {
        let mut cursor = Cursor::new(data);
        let relation_id = cursor.read_u32::<BigEndian>()?;
        let tuple_type = cursor.read_u8()?;

        if tuple_type != b'O' && tuple_type != b'K' {
            return Err(MeiliBridgeError::Source(format!(
                "Expected 'O' or 'K' tuple type for DELETE, got {}",
                tuple_type as char
            )));
        }

        let old_tuple = self.parse_tuple_data(&mut cursor)?;

        Ok(DeleteMessage {
            relation_id,
            old_tuple,
        })
    }

    fn parse_truncate(&self, data: &[u8]) -> Result<TruncateMessage> {
        let mut cursor = Cursor::new(data);
        let relation_count = cursor.read_u32::<BigEndian>()?;
        let options = cursor.read_u8()?;

        let mut relation_ids = Vec::with_capacity(relation_count as usize);
        for _ in 0..relation_count {
            relation_ids.push(cursor.read_u32::<BigEndian>()?);
        }

        Ok(TruncateMessage {
            relation_count,
            options,
            relation_ids,
        })
    }

    fn parse_keepalive(&self, data: &[u8]) -> Result<KeepaliveMessage> {
        let mut cursor = Cursor::new(data);
        let wal_end = cursor.read_u64::<BigEndian>()?;
        let timestamp = cursor.read_i64::<BigEndian>()?;
        let reply_requested = cursor.read_u8()? != 0;

        Ok(KeepaliveMessage {
            wal_end,
            timestamp,
            reply_requested,
        })
    }

    fn parse_tuple_data(&self, cursor: &mut Cursor<&[u8]>) -> Result<TupleData> {
        let column_count = cursor.read_u16::<BigEndian>()?;
        let mut columns = Vec::with_capacity(column_count as usize);

        for _ in 0..column_count {
            let tuple_type = cursor.read_u8()?;
            match tuple_type {
                b'n' => columns.push(None), // NULL
                b't' => {
                    // Text format
                    let len = cursor.read_u32::<BigEndian>()? as usize;
                    let mut data = vec![0u8; len];
                    cursor.read_exact(&mut data)?;
                    columns.push(Some(data));
                }
                b'u' => columns.push(None), // TOAST, unchanged
                _ => {
                    return Err(MeiliBridgeError::Source(format!(
                        "Unknown tuple data type: {}",
                        tuple_type as char
                    )));
                }
            }
        }

        Ok(TupleData { columns })
    }

    fn read_string(&self, cursor: &mut Cursor<&[u8]>) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            let byte = cursor.read_u8()?;
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        String::from_utf8(bytes)
            .map_err(|e| MeiliBridgeError::Source(format!("Invalid UTF-8 string: {e}")))
    }

    pub fn get_relation(&self, relation_id: u32) -> Option<&RelationMessage> {
        self.relations.get(&relation_id)
    }
}

impl Default for PgOutputParser {
    fn default() -> Self {
        Self::new()
    }
}
