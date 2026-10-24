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

    #[cfg(test)]
    pub fn power(n: u32) -> Self {
        Self(GROUP.g.modpow(&BigUint::from(n), &GROUP.p))
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

#[cfg(test)]
pub struct Phone {
    code: String,
    x: BigUint,
    pub pa: Vec<u8>,
}

#[cfg(test)]
impl Phone {
    pub fn new(code: &str, x: u64) -> Self {
        Self::with(code, BigUint::from(x))
    }

    fn with(code: &str, x: BigUint) -> Self {
        let g = &*GROUP;
        let (w, _) = secret(code);
        let pa = bytes(&(g.g.modpow(&x, &g.p) * g.m.modpow(&w, &g.p) % &g.p));
        Self {
            code: code.to_owned(),
            x,
            pa,
        }
    }

    pub fn confirms(&self, pb: &[u8], door: &[u8], confirm: &[u8]) -> bool {
        let g = &*GROUP;
        let (w, w_bytes) = secret(&self.code);
        let Some(pb_e) = Element::parse(pb) else {
            return false;
        };
        let k = (pb_e.0 * inverse(&g.n.modpow(&w, &g.p)) % &g.p).modpow(&self.x, &g.p);
        confirm == confirm_of(&[LABEL, &self.pa, pb, &bytes(&k), &w_bytes, door])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR: &str = include_str!("../android/app/src/test/resources/pake-vector.txt");

    fn field(name: &str) -> String {
        VECTOR
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("{name}"))
            .to_owned()
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn shared_vector_matches() {
        let code = field("code");
        let phone = Phone::with(&code, BigUint::from_bytes_be(&hex(&field("x"))));
        let y = BigUint::from_bytes_be(&hex(&field("y")));
        let door = field("door");
        assert_eq!(to_hex(&phone.pa), field("pa"));
        let answer = answer_with(
            &code,
            &Element::parse(&phone.pa).unwrap(),
            door.as_bytes(),
            &y,
        );
        assert_eq!(to_hex(&answer.pb), field("pb"));
        assert_eq!(to_hex(&answer.confirm), field("c"));
        assert!(phone.confirms(&answer.pb, door.as_bytes(), &answer.confirm));
    }

    #[test]
    fn wrong_code_or_door_fails() {
        let phone = Phone::new("ABCD2345", 123456789);
        let pa = Element::parse(&phone.pa).unwrap();
        let real = answer("ABCD2345", &pa, b"fp=A");
        assert!(phone.confirms(&real.pb, b"fp=A", &real.confirm));
        assert!(!phone.confirms(&real.pb, b"fp=B", &real.confirm));
        let guess = Phone::new("ABCD2346", 123456789);
        assert!(!guess.confirms(&real.pb, b"fp=A", &real.confirm));
        let other = answer("ABCD2346", &pa, b"fp=A");
        assert!(!phone.confirms(&other.pb, b"fp=A", &other.confirm));
    }

    #[test]
    fn bad_elements_are_refused() {
        let p = &GROUP.p;
        for bad in [
            BigUint::from(0u32),
            BigUint::from(1u32),
            p - BigUint::from(1u32),
            p.clone(),
        ] {
            assert!(Element::parse(&bytes(&bad)).is_none());
        }
        assert!(Element::parse(&bytes(&(p - BigUint::from(2u32)))).is_none());
        assert!(Element::parse(&[1u8; ELEMENT_LEN - 1]).is_none());
        assert!(Element::parse(&bytes(&GROUP.g)).is_some());
    }
}
