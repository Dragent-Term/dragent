//! Decoded VT operations shared by the daemon parser and display interpreters.
//! Printable runs are packed together rather than sending one object per cell.
use crate::parser::VtActor;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Text(String),
    Control(u8),
    Csi {
        params: Vec<u16>,
        private: Option<u8>,
        intermediates: Vec<u8>,
        action: u8,
    },
    Esc {
        intermediates: Vec<u8>,
        byte: u8,
    },
    Osc(Vec<u8>),
    Apc(Vec<u8>),
    Dcs(Vec<u8>),
}

impl Operation {
    pub fn apply(&self, actor: &mut impl VtActor) {
        match self {
            Self::Text(text) => {
                for ch in text.chars() {
                    actor.print(ch);
                }
            }
            Self::Control(byte) => actor.control(*byte),
            Self::Csi {
                params,
                private,
                intermediates,
                action,
            } => actor.csi(params, *private, intermediates, *action),
            Self::Esc {
                intermediates,
                byte,
            } => actor.esc(intermediates, *byte),
            Self::Osc(bytes) => actor.osc(bytes),
            Self::Apc(bytes) => actor.apc(bytes),
            Self::Dcs(bytes) => actor.dcs(bytes),
        }
    }

    /// Conservative owned-memory estimate used for queue and replay budgets.
    pub fn memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Text(text) => text.capacity(),
                Self::Control(_) => 0,
                Self::Csi {
                    params,
                    intermediates,
                    ..
                } => params.capacity() * 2 + intermediates.capacity(),
                Self::Esc { intermediates, .. } => intermediates.capacity(),
                Self::Osc(bytes) | Self::Apc(bytes) | Self::Dcs(bytes) => bytes.capacity(),
            }
    }
}

pub(crate) struct Recorder<'a, T> {
    pub actor: &'a mut T,
    pub operations: Vec<Operation>,
}

impl<T: VtActor> VtActor for Recorder<'_, T> {
    fn print(&mut self, ch: char) {
        self.actor.print(ch);
        if let Some(Operation::Text(text)) = self.operations.last_mut() {
            text.push(ch);
        } else {
            self.operations.push(Operation::Text(ch.to_string()));
        }
    }
    fn control(&mut self, byte: u8) {
        self.actor.control(byte);
        self.operations.push(Operation::Control(byte));
    }
    fn csi(&mut self, params: &[u16], private: Option<u8>, intermediates: &[u8], action: u8) {
        self.actor.csi(params, private, intermediates, action);
        self.operations.push(Operation::Csi {
            params: params.to_vec(),
            private,
            intermediates: intermediates.to_vec(),
            action,
        });
    }
    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        self.actor.esc(intermediates, byte);
        self.operations.push(Operation::Esc {
            intermediates: intermediates.to_vec(),
            byte,
        });
    }
    fn osc(&mut self, bytes: &[u8]) {
        self.actor.osc(bytes);
        self.operations.push(Operation::Osc(bytes.to_vec()));
    }
    fn apc(&mut self, bytes: &[u8]) {
        self.actor.apc(bytes);
        self.operations.push(Operation::Apc(bytes.to_vec()));
    }
    fn dcs(&mut self, bytes: &[u8]) {
        self.actor.dcs(bytes);
        self.operations.push(Operation::Dcs(bytes.to_vec()));
    }
}
