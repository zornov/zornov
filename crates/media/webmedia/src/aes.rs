// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AES-128 in CBC mode, for HLS segments whose playlist gives the key
//! openly (`METHOD=AES-128`). Straightforward table-free code; segments
//! are small, so speed is not the point.

pub struct Aes128 {
    rk: [[u8; 16]; 11],
    #[cfg_attr(not(test), allow(dead_code))]
    sbox: [u8; 256],
    inv: [u8; 256],
}

fn make_sbox() -> ([u8; 256], [u8; 256]) {
    let mut sbox = [0u8; 256];
    let (mut p, mut q) = (1u8, 1u8);
    loop {
        p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1B } else { 0 };
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        sbox[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    sbox[0] = 0x63;
    let mut inv = [0u8; 256];
    for (i, &s) in sbox.iter().enumerate() {
        inv[s as usize] = i as u8;
    }
    (sbox, inv)
}

fn xtime(x: u8) -> u8 {
    (x << 1) ^ if x & 0x80 != 0 { 0x1B } else { 0 }
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    r
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Aes128 {
        let (sbox, inv) = make_sbox();
        let mut w = [[0u8; 4]; 44];
        for i in 0..4 {
            w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        let mut rcon = 1u8;
        for i in 4..44 {
            let mut t = w[i - 1];
            if i % 4 == 0 {
                t = [sbox[t[1] as usize] ^ rcon, sbox[t[2] as usize], sbox[t[3] as usize], sbox[t[0] as usize]];
                rcon = xtime(rcon);
            }
            for j in 0..4 {
                w[i][j] = w[i - 4][j] ^ t[j];
            }
        }
        let mut rk = [[0u8; 16]; 11];
        for (r, k) in rk.iter_mut().enumerate() {
            for c in 0..4 {
                k[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
        }
        Aes128 { rk, sbox, inv }
    }

    fn add_key(s: &mut [u8; 16], k: &[u8; 16]) {
        for i in 0..16 {
            s[i] ^= k[i];
        }
    }

    #[cfg(test)]
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_key(block, &self.rk[0]);
        for round in 1..=10 {
            for b in block.iter_mut() {
                *b = self.sbox[*b as usize];
            }
            let old = *block;
            for c in 0..4 {
                for r in 0..4 {
                    block[c * 4 + r] = old[((c + r) % 4) * 4 + r];
                }
            }
            if round != 10 {
                for c in 0..4 {
                    let col = [block[c * 4], block[c * 4 + 1], block[c * 4 + 2], block[c * 4 + 3]];
                    block[c * 4] = xtime(col[0]) ^ (xtime(col[1]) ^ col[1]) ^ col[2] ^ col[3];
                    block[c * 4 + 1] = col[0] ^ xtime(col[1]) ^ (xtime(col[2]) ^ col[2]) ^ col[3];
                    block[c * 4 + 2] = col[0] ^ col[1] ^ xtime(col[2]) ^ (xtime(col[3]) ^ col[3]);
                    block[c * 4 + 3] = (xtime(col[0]) ^ col[0]) ^ col[1] ^ col[2] ^ xtime(col[3]);
                }
            }
            Self::add_key(block, &self.rk[round]);
        }
    }

    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        Self::add_key(block, &self.rk[10]);
        for round in (0..10).rev() {
            let old = *block;
            for c in 0..4 {
                for r in 0..4 {
                    block[((c + r) % 4) * 4 + r] = old[c * 4 + r];
                }
            }
            for b in block.iter_mut() {
                *b = self.inv[*b as usize];
            }
            Self::add_key(block, &self.rk[round]);
            if round != 0 {
                for c in 0..4 {
                    let col = [block[c * 4], block[c * 4 + 1], block[c * 4 + 2], block[c * 4 + 3]];
                    for r in 0..4 {
                        block[c * 4 + r] = gmul(col[r], 14) ^ gmul(col[(r + 1) % 4], 11) ^ gmul(col[(r + 2) % 4], 13) ^ gmul(col[(r + 3) % 4], 9);
                    }
                }
            }
        }
    }

    /// CBC decryption with PKCS#7 padding removed. A length that is not a
    /// multiple of 16 is an error.
    pub fn cbc_decrypt(&self, iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
        if data.is_empty() || data.len() % 16 != 0 {
            return Err("the segment's length is not a whole number of cipher blocks".into());
        }
        let mut out = Vec::with_capacity(data.len());
        let mut prev = *iv;
        for chunk in data.chunks_exact(16) {
            let mut b = [0u8; 16];
            b.copy_from_slice(chunk);
            let cipher = b;
            self.decrypt_block(&mut b);
            for i in 0..16 {
                b[i] ^= prev[i];
            }
            prev = cipher;
            out.extend_from_slice(&b);
        }
        let pad = *out.last().unwrap_or(&0) as usize;
        if (1..=16).contains(&pad) && out.len() >= pad && out[out.len() - pad..].iter().all(|&b| b as usize == pad) {
            out.truncate(out.len() - pad);
            Ok(out)
        } else {
            Err("the segment did not decrypt with the key the playlist gives".into())
        }
    }

    #[cfg(test)]
    pub fn cbc_encrypt(&self, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
        let pad = 16 - data.len() % 16;
        let mut padded = data.to_vec();
        padded.extend(std::iter::repeat_n(pad as u8, pad));
        let mut out = Vec::with_capacity(padded.len());
        let mut prev = *iv;
        for chunk in padded.chunks_exact(16) {
            let mut b = [0u8; 16];
            for i in 0..16 {
                b[i] = chunk[i] ^ prev[i];
            }
            self.encrypt_block(&mut b);
            prev = b;
            out.extend_from_slice(&b);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_fips_197_example() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let aes = Aes128::new(&key);
        let mut block: [u8; 16] = std::array::from_fn(|i| (i as u8) * 0x11);
        aes.encrypt_block(&mut block);
        assert_eq!(block, [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a]);
        aes.decrypt_block(&mut block);
        assert_eq!(block, std::array::from_fn::<u8, 16, _>(|i| (i as u8) * 0x11));
    }

    #[test]
    fn cbc_round_trips_and_checks_padding() {
        let aes = Aes128::new(&[7; 16]);
        let iv = [9u8; 16];
        for n in [0usize, 1, 15, 16, 17, 100] {
            let data: Vec<u8> = (0..n).map(|i| i as u8).collect();
            let enc = aes.cbc_encrypt(&iv, &data);
            assert_eq!(aes.cbc_decrypt(&iv, &enc).unwrap(), data);
        }
        let wrong = Aes128::new(&[8; 16]);
        let enc = aes.cbc_encrypt(&iv, b"hello world, hello world");
        assert!(wrong.cbc_decrypt(&iv, &enc).is_err());
        assert!(aes.cbc_decrypt(&iv, &enc[..20]).is_err());
    }
}
