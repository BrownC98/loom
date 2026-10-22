//! 짝짓기 코드로 PC를 확인하는 PAKE. 근거: docs/design.md 「원격」(2026-10-22 사람 결정 Q426 B).
//!
//! SPAKE2(RFC 9382)의 꼴을 RFC 3526 2048비트 MODP 군(차수 q=(p-1)/2인 부분군, g=2)에서 쓴다.
//! 타원 곡선(RustCrypto `spake2`의 Ed25519, P-256)을 고르지 않았다: Android 쪽(android/.../Pake.kt)이 곡선의 점 더하기를
//! 하려면 BouncyCastle을 싣거나 곡선 산술을 손으로 옮겨야 한다. MODP 군은 양쪽 모두 큰 정수 거듭제곱 하나로 된다.
//! 폰이 `pA`를 보내고 PC가 `pB`와 확인값을 돌려준다. 코드를 모르면 확인값을 만들지 못한다. 엿보거나 가운데에 낀 기기는
//! 한 번 보낼 때 코드 하나만 맞혀 볼 수 있다(코드를 오프라인으로 맞혀 볼 값을 얻지 못한다).
//! 같은 계산은 android/app/src/main/kotlin/io/github/brownc98/loom/Pake.kt에 있다. 두 쪽은 같은 시험 값(`VECTOR`)을 쓴다.

use std::sync::LazyLock;

use num_bigint::BigUint;
use ring::hmac;
use sha2::{Digest, Sha256};

/// 군 원소의 바이트 길이(큰 끝, 앞을 0으로 채운다).
pub const ELEMENT_LEN: usize = 256;
/// RFC 3526 「2048-bit MODP Group」(id 14)의 p.
const P_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF";
/// 이름표. 같은 해시를 다른 값에 다시 쓰지 않게 값마다 다르다.
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

/// 이산 로그를 아무도 모르는 원소: 해시를 늘려 p보다 긴 수를 만들고 제곱해 부분군에 넣는다.
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

/// 확인을 마친 상대 원소: 1과 p-1이 아니고 차수 q인 부분군에 있다.
pub struct Element(BigUint);

impl Element {
    /// 길이가 다르거나 부분군 밖이면 `None`이다.
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

    /// 시험용 원소 `g^n`.
    #[cfg(test)]
    pub fn power(n: u32) -> Self {
        Self(GROUP.g.modpow(&BigUint::from(n), &GROUP.p))
    }
}

/// PC가 돌려주는 값.
pub struct Answer {
    pub pb: Vec<u8>,
    pub confirm: Vec<u8>,
}

/// PC 쪽: 폰의 `pA`에 답한다. `door`(폰 문 지문과 주소)를 확인값에 묶는다. 폰은 확인값이 맞을 때만 그 지문을 믿는다.
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

/// 코드에서 만든 비밀 `w`와 그 바이트. 코드는 대문자 8자다.
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

/// RFC 9382의 TT처럼 값마다 8바이트(작은 끝) 길이를 앞에 붙여 잇는다. 키는 TT의 SHA-256, 확인값은 그 키의 HMAC이다.
fn confirm_of(parts: &[&[u8]]) -> Vec<u8> {
    let mut tt = Sha256::new();
    for part in parts {
        tt.update((part.len() as u64).to_le_bytes());
        tt.update(part);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, &tt.finalize());
    hmac::sign(&key, LABEL_CONFIRM).as_ref().to_vec()
}

/// 폰 쪽 계산(android/.../Pake.kt와 같다). 시험에서만 쓴다.
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

    /// 양쪽이 함께 쓰는 시험 값. Kotlin 시험(android/app/src/test/.../PakeTest.kt)도 이 파일을 읽는다.
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
        // 부분군 밖: g=2는 이차 잉여이고, p ≡ 7 (mod 8)이라 -2(= p-2)는 이차 비잉여다.
        assert!(Element::parse(&bytes(&(p - BigUint::from(2u32)))).is_none());
        assert!(Element::parse(&[1u8; ELEMENT_LEN - 1]).is_none());
        assert!(Element::parse(&bytes(&GROUP.g)).is_some());
    }
}
