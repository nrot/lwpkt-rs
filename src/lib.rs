mod ffi;

pub struct LwRb {
    lwrb: ffi::lwrb,
    _buffer: Vec<u8>,
}

impl LwRb {
    pub fn new(size: usize) -> Self {
        let mut lwrb = ffi::lwrb::default();

        let mut buffer = vec![0u8; size];

        let res = unsafe {
            ffi::lwrb_init(
                &mut lwrb as *mut _,
                buffer.as_mut_ptr() as *mut _,
                buffer.len(),
            )
        };

        debug_assert_eq!(res, 1);

        Self { lwrb, _buffer: buffer }
    }
}

#[derive(Debug)]
pub enum Error {
    ERR = 0x1,
    INPROG,
    VALID,
    ERRCRC,
    ERRSTOP,
    WAITDATA,
    ERRMEM,
}

impl From<ffi::lwpktr_t::Type> for Error {
    fn from(value: ffi::lwpktr_t::Type) -> Self {
        match value {
            ffi::lwpktr_t::lwpktERR => Self::ERR,
            ffi::lwpktr_t::lwpktINPROG => Self::INPROG,
            ffi::lwpktr_t::lwpktVALID => Self::VALID,
            ffi::lwpktr_t::lwpktERRCRC => Self::ERRCRC,
            ffi::lwpktr_t::lwpktERRSTOP => Self::ERRSTOP,
            ffi::lwpktr_t::lwpktWAITDATA => Self::WAITDATA,
            ffi::lwpktr_t::lwpktERRMEM => Self::ERRMEM,
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
    lwpkt: ffi::lwpkt,
    read_buffer: LwRb,
    write_buffer: LwRb,
}

impl LwPkt {
    pub const MAX_PACKAGE_SIZE: u32 = ffi::LWPKT_CFG_MAX_DATA_LEN;

    pub fn new(read_buffer: LwRb, write_buffer: LwRb) -> Result<Self, Error> {
        let lwpkt = ffi::lwpkt::default();

        let mut result = Self {
            lwpkt,
            read_buffer,
            write_buffer,
        };

        let res = unsafe {
            ffi::lwpkt_init(
                &mut result.lwpkt as *mut _,
                &mut result.write_buffer.lwrb as *mut _,
                &mut result.read_buffer.lwrb as *mut _,
            )
        };
        check_result(res)?;

        Ok(result)
    }

    pub fn set_addres(&mut self, address: u8) -> Result<(), Error> {
        let res = unsafe { ffi::lwpkt_set_addr(&mut self.lwpkt as *mut _, address) };

        check_result(res)
    }

    pub fn write(&mut self, address: u8, command: u32, data: &[u8]) -> Result<(), Error> {
        let res = unsafe {
            ffi::lwpkt_write(
                &mut self.lwpkt as *mut _,
                address,
                command,
                data.as_ptr() as *mut _,
                data.len(),
            )
        };

        check_result(res)
    }

    pub fn read(&mut self) -> Result<(u32, &[u8]), Error> {
        let res = unsafe { ffi::lwpkt_read(&mut self.lwpkt as *mut _) };

        check_result(res)?;

        Ok((self.get_cmd(), self.get_data()))
    }

    ///
    /// /**
    ///  * \brief           Get pointer to packet data
    ///  * \param[in]       pkt: LwPKT instance
    ///  * \return          Pointer to data
    ///  */
    /// #define lwpkt_get_data(pkt)      (void*)(((pkt) != NULL) ? ((pkt)->data) : NULL)
    fn get_data(&self) -> &[u8] {
        &self.lwpkt.data
    }

    fn get_cmd(&self) -> u32 {
        self.lwpkt.m.cmd
    }

    pub fn raw_write(&mut self, raw: &[u8])->Result<(), Error>{
        let res = unsafe{
            ffi::lwrb_write(&mut self.write_buffer.lwrb as *mut _, raw.as_ptr() as *mut _, raw.len())
        };

        todo!()
    }

}


#[cfg(test)]
mod test{
    use crate::{LwPkt, LwRb};

    #[test]
    fn init_test(){
        let rb = LwRb::new(1024);
        let wb = LwRb::new(1024);

        let mut lwpkt = LwPkt::new(rb, wb).unwrap();

        lwpkt.set_addres(0x12).unwrap();

        lwpkt.write(0x11, 0x85, b"some hello").unwrap();
    }
}
