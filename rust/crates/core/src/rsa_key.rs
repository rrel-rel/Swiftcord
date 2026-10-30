//! The RSA key pair for QR login: 2048-bit, OAEP with SHA-256. On Windows it's CNG's (bcrypt.dll), so the
//! big-integer maths isn't compiled in; elsewhere it's the `rsa` crate's.

pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[cfg(windows)]
pub use cng::KeyPair;
#[cfg(not(windows))]
pub use rust::KeyPair;

/// DER for a SubjectPublicKeyInfo holding an RSA public key (big-endian modulus and exponent).
#[cfg_attr(not(windows), allow(dead_code))]
fn spki_der(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let len = content.len();
        if len < 0x80 {
            out.push(len as u8);
        } else {
            let bytes: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
            out.push(0x80 | bytes.len() as u8);
            out.extend_from_slice(&bytes);
        }
        out.extend_from_slice(content);
        out
    }
    fn integer(n: &[u8]) -> Vec<u8> {
        let n: Vec<u8> = n.iter().copied().skip_while(|&b| b == 0).collect();
        // A set top bit would read as negative.
        let n = if n.first().is_none_or(|&b| b & 0x80 != 0) { [&[0u8][..], &n].concat() } else { n };
        tlv(0x02, &n)
    }
    const RSA_ENCRYPTION: &[u8] = &[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00];
    let key = tlv(0x30, &[integer(modulus), integer(exponent)].concat());
    let bits = tlv(0x03, &[&[0u8][..], &key].concat());
    tlv(0x30, &[tlv(0x30, RSA_ENCRYPTION), bits].concat())
}

#[cfg(windows)]
mod cng {
    use windows::Win32::Foundation::NTSTATUS;
    use windows::Win32::Security::Cryptography::*;

    use super::BoxErr;

    fn check(status: NTSTATUS) -> Result<(), BoxErr> {
        if status.is_ok() { Ok(()) } else { Err(format!("CNG error {:#x}", status.0).into()) }
    }

    pub struct KeyPair {
        algorithm: BCRYPT_ALG_HANDLE,
        key: BCRYPT_KEY_HANDLE,
    }

    // CNG key handles may be used from any thread.
    unsafe impl Send for KeyPair {}
    unsafe impl Sync for KeyPair {}

    fn oaep() -> BCRYPT_OAEP_PADDING_INFO {
        BCRYPT_OAEP_PADDING_INFO { pszAlgId: BCRYPT_SHA256_ALGORITHM, pbLabel: std::ptr::null_mut(), cbLabel: 0 }
    }

    impl KeyPair {
        pub fn generate() -> Result<KeyPair, BoxErr> {
            let mut algorithm = BCRYPT_ALG_HANDLE::default();
            unsafe { check(BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_RSA_ALGORITHM, None, Default::default()))? };
            let mut pair = KeyPair { algorithm, key: BCRYPT_KEY_HANDLE::default() };
            unsafe {
                check(BCryptGenerateKeyPair(algorithm, &mut pair.key, 2048, 0))?;
                check(BCryptFinalizeKeyPair(pair.key, 0))?;
            }
            Ok(pair)
        }

        /// The public key as DER (SubjectPublicKeyInfo).
        pub fn public_key_der(&self) -> Result<Vec<u8>, BoxErr> {
            let mut size = 0;
            unsafe { check(BCryptExportKey(self.key, None, BCRYPT_RSAPUBLIC_BLOB, None, &mut size, 0))? };
            let mut blob = vec![0u8; size as usize];
            unsafe { check(BCryptExportKey(self.key, None, BCRYPT_RSAPUBLIC_BLOB, Some(&mut blob), &mut size, 0))? };
            // A BCRYPT_RSAKEY_BLOB header, then the exponent and the modulus, big-endian.
            let field = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap()) as usize;
            let (exp_len, mod_len) = (field(8), field(12));
            let header = size_of::<BCRYPT_RSAKEY_BLOB>();
            let exponent = &blob[header..header + exp_len];
            let modulus = &blob[header + exp_len..header + exp_len + mod_len];
            Ok(super::spki_der(modulus, exponent))
        }

        pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, BoxErr> {
            let padding = oaep();
            let padding = Some(&padding as *const _ as *const core::ffi::c_void);
            let mut size = 0;
            unsafe { check(BCryptDecrypt(self.key, Some(ciphertext), padding, None, None, &mut size, BCRYPT_PAD_OAEP))? };
            let mut out = vec![0u8; size as usize];
            unsafe { check(BCryptDecrypt(self.key, Some(ciphertext), padding, None, Some(&mut out), &mut size, BCRYPT_PAD_OAEP))? };
            out.truncate(size as usize);
            Ok(out)
        }

        #[cfg(test)]
        pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, BoxErr> {
            let padding = oaep();
            let padding = Some(&padding as *const _ as *const core::ffi::c_void);
            let mut size = 0;
            unsafe { check(BCryptEncrypt(self.key, Some(plaintext), padding, None, None, &mut size, BCRYPT_PAD_OAEP))? };
            let mut out = vec![0u8; size as usize];
            unsafe { check(BCryptEncrypt(self.key, Some(plaintext), padding, None, Some(&mut out), &mut size, BCRYPT_PAD_OAEP))? };
            Ok(out)
        }
    }

    impl Drop for KeyPair {
        fn drop(&mut self) {
            unsafe {
                let _ = BCryptDestroyKey(self.key);
                let _ = BCryptCloseAlgorithmProvider(self.algorithm, 0);
            }
        }
    }
}

#[cfg(not(windows))]
mod rust {
    use rsa::pkcs8::EncodePublicKey;
    use rsa::{Oaep, RsaPrivateKey};
    use sha2::Sha256;

    use super::BoxErr;

    pub struct KeyPair(RsaPrivateKey);

    impl KeyPair {
        pub fn generate() -> Result<KeyPair, BoxErr> {
            Ok(KeyPair(RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?))
        }

        pub fn public_key_der(&self) -> Result<Vec<u8>, BoxErr> {
            Ok(self.0.to_public_key().to_public_key_der()?.as_bytes().to_vec())
        }

        pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, BoxErr> {
            Ok(self.0.decrypt(Oaep::new::<Sha256>(), ciphertext)?)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_exports_a_public_key() {
        let key = KeyPair::generate().unwrap();
        let der = key.public_key_der().unwrap();
        // SEQUENCE { SEQUENCE { rsaEncryption, NULL }, BIT STRING { … } }, 294 bytes for a 2048-bit key.
        assert_eq!(der.len(), 294);
        assert_eq!(&der[..4], &[0x30, 0x82, 0x01, 0x22]);
        assert_eq!(&der[6..17], &[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01]);
        let secret = b"a nonce from the remote auth gateway";
        assert_eq!(key.decrypt(&key.encrypt(secret).unwrap()).unwrap(), secret);
    }
}
