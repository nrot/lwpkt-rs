use std::pin::Pin;

use async_channel::{Receiver, Sender};

mod ffi;

pub struct LwRb {
    lwrb: ffi::lwrb,
    buffer: Pin<Vec<u8>>,
}

impl LwRb {
    pub fn new(size: usize) -> Self {
        let mut lwrb = ffi::lwrb::default();

        let mut buffer = Pin::new(vec![0u8; size]);

        let res = unsafe {
            ffi::lwrb_init(
                &mut lwrb as *mut _,
                buffer.as_mut_ptr() as *mut _,
                buffer.len(),
            )
        };

        debug_assert_eq!(res, 1);

        Self { lwrb, buffer }
    }

    pub fn size(&self) -> usize {
        self.buffer.len()
    }
}

#[derive(Debug)]
pub enum Error {
    ERR = 0x1,
    InProgress,
    Valid,
    ErrorCRC,
    ErrStop,
    WaitData,
    ErrorMem,
    ErrorClosedRaw,
}

impl From<ffi::lwpktr_t::Type> for Error {
    fn from(value: ffi::lwpktr_t::Type) -> Self {
        match value {
            ffi::lwpktr_t::lwpktERR => Self::ERR,
            ffi::lwpktr_t::lwpktINPROG => Self::InProgress,
            ffi::lwpktr_t::lwpktVALID => Self::Valid,
            ffi::lwpktr_t::lwpktERRCRC => Self::ErrorCRC,
            ffi::lwpktr_t::lwpktERRSTOP => Self::ErrStop,
            ffi::lwpktr_t::lwpktWAITDATA => Self::WaitData,
            ffi::lwpktr_t::lwpktERRMEM => Self::ErrorMem,
            e => {
                panic!("Unknow type error: {e}")
            }
        }
    }
}

fn check_result(res: u32) -> Result<(), Error> {
    if res == ffi::lwpktr_t::lwpktOK {
        Ok(())
    } else {
        Err(res.into())
    }
}

pub struct LwPkt<const RAW_SIZE: usize> {
    lwpkt: Pin<Box<ffi::lwpkt>>,
    read_buffer: Pin<Box<LwRb>>,
    write_buffer: Pin<Box<LwRb>>,

    buffer: [u8; RAW_SIZE],
    to_raw: Sender<u8>,
    from_raw: Receiver<u8>,
}

#[derive(Clone)]
pub struct LwPktRaw {
    to_pkt: Sender<u8>,
    from_pkt: Receiver<u8>,
}

#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Package<'a> {
    cmd: u32,
    from: u8,
    to: u8,
    data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status<'a> {
    InProgress,
    WaitData,
    Valid(Package<'a>),
}

impl<const RAW_SIZE: usize> LwPkt<RAW_SIZE> {
    pub const MAX_PACKAGE_SIZE: u32 = ffi::LWPKT_CFG_MAX_DATA_LEN;

    pub fn new(read_buffer: LwRb, write_buffer: LwRb) -> Result<(Self, LwPktRaw), Error> {
        let lwpkt = Box::pin(ffi::lwpkt::default());

        let (tx_to_raw, rx_to_raw) = async_channel::bounded(RAW_SIZE);
        let (tx_to_pkt, rx_to_pkt) = async_channel::bounded(RAW_SIZE);

        let mut result = Self {
            lwpkt,
            read_buffer: Box::pin(read_buffer),
            write_buffer: Box::pin(write_buffer),
            buffer: [0u8; RAW_SIZE],
            to_raw: tx_to_raw,
            from_raw: rx_to_pkt,
        };

        let res = unsafe {
            ffi::lwpkt_init(
                result.lwpkt.as_mut().get_mut() as *mut _,
                &mut result.write_buffer.lwrb as *mut _,
                &mut result.read_buffer.lwrb as *mut _,
            )
        };
        check_result(res)?;

        let raw = LwPktRaw {
            to_pkt: tx_to_pkt,
            from_pkt: rx_to_raw,
        };

        Ok((result, raw))
    }

    pub fn set_addres(&mut self, address: u8) -> Result<(), Error> {
        let res = unsafe { ffi::lwpkt_set_addr(self.lwpkt.as_mut().get_mut() as *mut _, address) };

        check_result(res)
    }

    pub fn write<'b>(&mut self, package: Package<'b>) -> Result<(), Error> {
        let res = unsafe {
            ffi::lwpkt_write(
                self.lwpkt.as_mut().get_mut() as *mut _,
                package.to,
                package.cmd,
                package.data.as_ptr() as *mut _,
                package.data.len(),
            )
        };

        check_result(res)?;

        let wb = &mut self.write_buffer.lwrb as *mut _;

        loop {
            let res = unsafe { ffi::lwrb_read(wb, self.buffer.as_mut_ptr() as *mut _, RAW_SIZE) };

            if res == 0 {
                break;
            }

            for b in &self.buffer[..res] {
                match self.to_raw.try_send(*b) {
                    Ok(_) => {}
                    Err(async_channel::TrySendError::Full(_)) => {
                        return Err(Error::ErrorMem);
                    }
                    Err(async_channel::TrySendError::Closed(_)) => {
                        return Err(Error::ErrorClosedRaw);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn read(&'_ mut self) -> Result<Status<'_>, Error> {
        let mut buffer_len = 0usize;
        for (i, b) in self.buffer.iter_mut().enumerate() {
            match self.from_raw.try_recv() {
                Ok(v) => *b = v,
                Err(async_channel::TryRecvError::Empty) => {
                    buffer_len = i;
                    break;
                }
                Err(_) => {
                    return Err(Error::ErrorClosedRaw);
                }
            }
        }

        if buffer_len > 0 {
            let res = unsafe {
                ffi::lwrb_write(
                    &mut self.read_buffer.lwrb as *mut _,
                    self.buffer.as_ptr() as *mut _,
                    buffer_len,
                )
            };

            if res != buffer_len {
                return Err(Error::ErrorMem);
            }
        }

        let res = unsafe { ffi::lwpkt_read(self.lwpkt.as_mut().get_mut() as *mut _) };

        match res {
            ffi::lwpktr_t::lwpktVALID => Ok(Status::Valid(Package {
                cmd: self.get_cmd(),
                data: self.get_data(),
                from: self.get_from(),
                to: self.get_to(),
            })),
            ffi::lwpktr_t::lwpktWAITDATA => Ok(Status::WaitData),
            ffi::lwpktr_t::lwpktINPROG => Ok(Status::InProgress),
            e => Err(e.into()),
        }
    }

    fn get_data(&self) -> &[u8] {
        let len = self.lwpkt.m.len;
        &self.lwpkt.data[..len]
    }

    fn get_cmd(&self) -> u32 {
        self.lwpkt.m.cmd
    }

    fn get_from(&self) -> u8 {
        self.lwpkt.m.from
    }

    fn get_to(&self) -> u8 {
        self.lwpkt.m.to
    }

    #[allow(dead_code)]
    fn raw_write(&mut self, raw: &[u8]) -> Result<(), Error> {
        let _res = unsafe {
            ffi::lwrb_write(
                &mut self.write_buffer.lwrb as *mut _,
                raw.as_ptr() as *mut _,
                raw.len(),
            )
        };

        todo!()
    }
}

impl std::io::Read for LwPktRaw {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        for (i, b) in buf.iter_mut().enumerate() {
            match self.from_pkt.try_recv() {
                Ok(v) => *b = v,
                Err(async_channel::TryRecvError::Empty) => return Ok(i),
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        e.to_string(),
                    ));
                }
            }
        }
        Ok(buf.len())
    }
}

impl std::io::Write for LwPktRaw {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for (i, b) in buf.iter().enumerate() {
            match self.to_pkt.try_send(*b) {
                Ok(()) => {}
                Err(async_channel::TrySendError::Full(_)) => {
                    return Ok(i);
                }
                Err(async_channel::TrySendError::Closed(v)) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        async_channel::TrySendError::Closed(v).to_string(),
                    ));
                }
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::io::{Read, Write};

    use crate::{LwPkt, LwRb, Status};

    #[test]
    fn init_test() {
        let rb = LwRb::new(1024);
        let wb = LwRb::new(1024);

        let (mut lwpkt, mut raw_pkt) = LwPkt::<1024>::new(rb, wb).unwrap();

        lwpkt.set_addres(0x12).unwrap();

        lwpkt
            .write(crate::Package {
                cmd: 0x85,
                from: 0,
                to: 0x11,
                data: b"some hello",
            })
            .unwrap();

        let mut buffer = vec![];
        raw_pkt.read_to_end(&mut buffer).unwrap();

        raw_pkt.write_all(&buffer).unwrap();

        let s = lwpkt.read().unwrap();

        assert_eq!(
            s,
            Status::Valid(crate::Package {
                cmd: 0x85,
                from: 0x12,
                to: 0x11,
                data: b"some hello"
            })
        )
    }
}
