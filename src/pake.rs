use std::sync::LazyLock;

use base64::Engine;
use num_bigint::BigUint;
use ring::hmac;
use sha2::{Digest, Sha256};

pub const ELEMENT_LEN: usize = 256;
const P_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF";
const LABEL: &[u8] = b"loom pake v1";
const LABEL_M: &[u8] = b"loom pake v1 M";
const LABEL_N: &[u8] = b"loom pake v1 N";
const LABEL_W: &[u8] = b"loom pake v1 w";
const LABEL_CONFIRM: &[u8] = b"loom pake v1 pc confirm";

struct Group {
    p: BigUint,
    q: BigUint,
    g: BigUint,
    m: BigUint,
    n: BigUint,
}

static GROUP: LazyLock<Group> = LazyLock::new(|| {
    let p = BigUint::parse_bytes(P_HEX.as_bytes(), 16).expect("p는 16진수다");
    let q = &p >> 1;
    Group {
        g: BigUint::from(2u32),
        m: to_group(&p, LABEL_M),
        n: to_group(&p, LABEL_N),
        p,
        q,
    }
});

fn to_group(p: &BigUint, label: &[u8]) -> BigUint {
    let mut wide = Vec::with_capacity(9 * 32);
    for i in 0..9u8 {
        wide.extend_from_slice(
            &Sha256::new()
                .chain_update(label)
                .chain_update([i])
                .finalize(),
        );
    }
    (BigUint::from_bytes_be(&wide) % p).modpow(&BigUint::from(2u32), p)
}

pub struct Element(BigUint);

impl Element {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let group = &*GROUP;
        if bytes.len() != ELEMENT_LEN {
            return None;
        }
        let e = BigUint::from_bytes_be(bytes);
        let one = BigUint::from(1u32);
        let ok = e > one && e < &group.p - &one && e.modpow(&group.q, &group.p) == one;
        ok.then_some(Self(e))
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(bytes(&self.0)).into()
    }
}

pub struct Answer {
    pub pb: Vec<u8>,
    pub confirm: Vec<u8>,
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS 난수를 읽지 못했다");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn answer(code: &str, pa: &Element, door: &[u8]) -> Answer {
    let mut wide = [0u8; ELEMENT_LEN];
    getrandom::fill(&mut wide).expect("OS 난수");
    answer_with(code, pa, door, &(BigUint::from_bytes_be(&wide) % &GROUP.q))
}

fn answer_with(code: &str, pa: &Element, door: &[u8], y: &BigUint) -> Answer {
    let group = &*GROUP;
    let (w, w_bytes) = secret(code);
    let pb = group.g.modpow(y, &group.p) * group.n.modpow(&w, &group.p) % &group.p;
    let unmasked = &pa.0 * inverse(&group.m.modpow(&w, &group.p)) % &group.p;
    let k = unmasked.modpow(y, &group.p);
    let (pa, pb, k) = (bytes(&pa.0), bytes(&pb), bytes(&k));
    let confirm = confirm_of(&[LABEL, &pa, &pb, &k, &w_bytes, door]);
    Answer { pb, confirm }
}

fn secret(code: &str) -> (BigUint, Vec<u8>) {
    let w = Sha256::new()
        .chain_update(LABEL_W)
        .chain_update(code.as_bytes())
        .finalize()
        .to_vec();
    (BigUint::from_bytes_be(&w) % &GROUP.q, w)
}

fn inverse(e: &BigUint) -> BigUint {
    let p = &GROUP.p;
    e.modpow(&(p - BigUint::from(2u32)), p)
}

fn bytes(e: &BigUint) -> Vec<u8> {
    let raw = e.to_bytes_be();
    let mut out = vec![0u8; ELEMENT_LEN - raw.len()];
    out.extend_from_slice(&raw);
    out
}

fn confirm_of(parts: &[&[u8]]) -> Vec<u8> {
    let mut tt = Sha256::new();
    for part in parts {
        tt.update((part.len() as u64).to_le_bytes());
        tt.update(part);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, &tt.finalize());
    hmac::sign(&key, LABEL_CONFIRM).as_ref().to_vec()
}
