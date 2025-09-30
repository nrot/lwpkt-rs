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

#[derive(Debug, PartialEq, Eq)]
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

pub struct LwPkt {
    lwpkt: Pin<Box<ffi::lwpkt>>,
    read_buffer: Pin<Box<LwRb>>,
    write_buffer: Pin<Box<LwRb>>,

    to_raw: Sender<Vec<u8>>,
    from_raw: Receiver<Vec<u8>>,
}

pub struct LwPktRaw {
    last_read: Vec<u8>,
    to_pkt: Sender<Vec<u8>>,
    from_pkt: Receiver<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Package {
    pub cmd: u32,
    pub from: u8,
    pub to: u8,
    pub data: Vec<u8>,
}

impl LwPkt {
    pub const MAX_PACKAGE_SIZE: u32 = ffi::LWPKT_CFG_MAX_DATA_LEN;

    pub fn new(read_buffer: LwRb, write_buffer: LwRb) -> Result<(Self, LwPktRaw), Error> {
        let lwpkt = Box::pin(ffi::lwpkt::default());

        let (tx_to_raw, rx_to_raw) = async_channel::bounded(64);
        let (tx_to_pkt, rx_to_pkt) = async_channel::bounded(64);

        let mut result = Self {
            lwpkt,
            read_buffer: Box::pin(read_buffer),
            write_buffer: Box::pin(write_buffer),
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
            last_read: Vec::new(),
            to_pkt: tx_to_pkt,
            from_pkt: rx_to_raw,
        };

        Ok((result, raw))
    }

    pub fn set_addres(&mut self, address: u8) -> Result<(), Error> {
        let res = unsafe { ffi::lwpkt_set_addr(self.lwpkt.as_mut().get_mut() as *mut _, address) };

        check_result(res)
    }

    pub fn write(&mut self, package: Package) -> Result<(), Error> {
        let res = unsafe {
            ffi::lwpkt_write(
                self.lwpkt.as_mut().get_mut() as *mut _,
                package.to,
                package.cmd as _,
                package.data.as_ptr() as *mut _,
                package.data.len(),
            )
        };

        check_result(res)?;

        let wb = &mut self.write_buffer.lwrb as *mut _;

        let mut buffer = vec![0u8; 1024];
        loop {
            let res = unsafe { ffi::lwrb_read(wb, buffer.as_mut_ptr() as *mut _, buffer.len()) };

            if res == 0 {
                break;
            }

            match self.to_raw.try_send((&buffer[..res]).to_vec()) {
                Ok(_) => {}
                Err(async_channel::TrySendError::Full(_)) => {
                    return Err(Error::ErrorMem);
                }
                Err(async_channel::TrySendError::Closed(_)) => {
                    return Err(Error::ErrorClosedRaw);
                }
            }
        }

        Ok(())
    }

    pub fn read(&'_ mut self) -> Result<Vec<Package>, Error> {
        let mut results = Vec::new();
        loop {
            match self.from_raw.try_recv() {
                Ok(buffer) => {
                    let mut from = 0;
                    while from < buffer.len() {
                        let res = unsafe {
                            ffi::lwrb_write(
                                &mut self.read_buffer.lwrb as *mut _,
                                (&buffer[from..]).as_ptr() as *mut _,
                                buffer.len() - from,
                            )
                        };

                        let status =
                            unsafe { ffi::lwpkt_read(self.lwpkt.as_mut().get_mut() as *mut _) };

                        match status {
                            ffi::lwpktr_t::lwpktVALID => {
                                results.push(Package {
                                    cmd: self.get_cmd(),
                                    data: self.get_data().to_vec(),
                                    from: self.get_from(),
                                    to: self.get_to(),
                                });
                            }
                            ffi::lwpktr_t::lwpktWAITDATA => {}
                            ffi::lwpktr_t::lwpktINPROG => {}
                            e => return Err(e.into()),
                        };

                        from += res;
                    }
                }
                Err(async_channel::TryRecvError::Empty) => {
                    break;
                }
                Err(_) => {
                    return Err(Error::ErrorClosedRaw);
                }
            }
        }

        Ok(results)
    }

    pub fn get_data(&self) -> &[u8] {
        let len = self.lwpkt.m.len;
        &self.lwpkt.data[..len]
    }

    pub fn get_cmd(&self) -> u32 {
        self.lwpkt.m.cmd as u32
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
        let mut readed = 0usize;
        if !self.last_read.is_empty() {
            match buf.len().cmp(&self.last_read.len()) {
                std::cmp::Ordering::Less => {
                    buf.copy_from_slice(&self.last_read[..buf.len()]);
                    self.last_read = self.last_read[buf.len()..].to_vec();
                    return Ok(buf.len());
                }
                std::cmp::Ordering::Equal => {
                    buf.copy_from_slice(&self.last_read);
                    self.last_read = Vec::new();
                    return Ok(buf.len());
                }
                std::cmp::Ordering::Greater => {
                    (&mut buf[..self.last_read.len()]).copy_from_slice(&self.last_read);
                    readed = self.last_read.len();
                    self.last_read = Vec::new();
                }
            }
        }

        loop {
            match self.from_pkt.try_recv() {
                Ok(src) => {
                    let buffer = &mut buf[readed..];
                    match buffer.len().cmp(&src.len()) {
                        std::cmp::Ordering::Less => {
                            buffer.copy_from_slice(&src[..buffer.len()]);
                            self.last_read = src[buffer.len()..].to_vec();
                            readed += buffer.len();
                            return Ok(readed);
                        }
                        std::cmp::Ordering::Equal => {
                            buffer.copy_from_slice(&src);
                            readed += buffer.len();
                            return Ok(readed);
                        }
                        std::cmp::Ordering::Greater => {
                            (&mut buffer[..src.len()]).copy_from_slice(&src);
                            readed += src.len();
                        }
                    }
                }
                Err(async_channel::TryRecvError::Empty) => break,
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        e.to_string(),
                    ));
                }
            }
        }

        Ok(readed)
    }
}

impl std::io::Write for LwPktRaw {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.to_pkt.try_send(buf.to_vec()) {
            Ok(()) => Ok(buf.len()),
            Err(async_channel::TrySendError::Full(_)) => Ok(0),
            Err(async_channel::TrySendError::Closed(v)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                async_channel::TrySendError::Closed(v).to_string(),
            )),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::io::{Read, Write};

    use crate::{LwPkt, LwRb};

    #[test]
    fn init_test() {
        let rb = LwRb::new(1024);
        let wb = LwRb::new(1024);

        let (mut lwpkt, mut raw_pkt) = LwPkt::new(rb, wb).unwrap();

        lwpkt.set_addres(0x12).unwrap();

        lwpkt
            .write(crate::Package {
                cmd: 0x85,
                from: 0,
                to: 0x11,
                data: b"some hello".to_vec(),
            })
            .unwrap();

        let mut buffer = vec![];
        raw_pkt.read_to_end(&mut buffer).unwrap();

        raw_pkt.write_all(&buffer).unwrap();

        let s = lwpkt.read().unwrap();

        assert_eq!(
            (*s.first().unwrap()),
            crate::Package {
                cmd: 0x85,
                from: 0x12,
                to: 0x11,
                data: b"some hello".to_vec()
            }
        )
    }
}
