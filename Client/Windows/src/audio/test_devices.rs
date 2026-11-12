//! Thread-local device factory: exercise real app events and audio workers without hardware.
use super::DeviceStream;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy)]
pub enum Kind {
    Input,
    Output,
}

#[derive(Default)]
pub struct Counts {
    pub inputs: usize,
    pub outputs: usize,
    pub input_starts: usize,
    pub fail_input: bool,
    pub fail_output: bool,
}

thread_local! {
    static DEVICES: RefCell<Option<Rc<RefCell<Counts>>>> = const { RefCell::new(None) };
}

pub struct Devices(pub Rc<RefCell<Counts>>);
impl Devices {
    pub fn new() -> Self {
        let counts = Rc::new(RefCell::new(Counts::default()));
        DEVICES.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(counts.clone());
        });
        Self(counts)
    }
}
impl Drop for Devices {
    fn drop(&mut self) {
        DEVICES.with(|slot| *slot.borrow_mut() = None);
        assert_eq!(self.0.borrow().inputs, 0);
        assert_eq!(self.0.borrow().outputs, 0);
    }
}

pub struct Stream {
    counts: Rc<RefCell<Counts>>,
    kind: Kind,
}
impl Drop for Stream {
    fn drop(&mut self) {
        let mut counts = self.counts.borrow_mut();
        match self.kind {
            Kind::Input => counts.inputs -= 1,
            Kind::Output => counts.outputs -= 1,
        }
    }
}

pub fn open(kind: Kind, name: Option<&str>) -> Option<anyhow::Result<DeviceStream>> {
    DEVICES.with(|slot| {
        let counts = slot.borrow().as_ref()?.clone();
        let mut state = counts.borrow_mut();
        let fail = match kind {
            Kind::Input => {
                state.input_starts += 1;
                state.fail_input
            }
            Kind::Output => state.fail_output,
        };
        if fail || name == Some("broken") {
            return Some(Err(anyhow::anyhow!("device unavailable")));
        }
        match kind {
            Kind::Input => {
                state.inputs += 1;
                assert_eq!(state.inputs, 1, "capture owners must not overlap");
            }
            Kind::Output => state.outputs += 1,
        }
        drop(state);
        Some(Ok(DeviceStream::Fake {
            _stream: Stream { counts, kind },
        }))
    })
}
