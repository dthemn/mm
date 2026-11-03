//! The key hierarchy is described in docs/ARCHITECTURE.md, section 2.
//! No home-grown primitives here, only composition of vetted crates.

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use p256::ecdsa::{
    Signature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
/// A 12-word phrase carries 128 bits of entropy.
const PHRASE_ENTROPY_LEN: usize = 16;
const BLOB_FORMAT: u8 = 1;

const INFO_KEK_PRF: &[u8] = b"mm/v1/kek/prf";
const INFO_KEK_PHRASE: &[u8] = b"mm/v1/kek/phrase";
const INFO_AUTH_PHRASE: &[u8] = b"mm/v1/auth/phrase";
const LABEL_RECORD: &[u8] = b"mm/v1/record";
const LABEL_WRAP: &[u8] = b"mm/v1/wrap";
const LABEL_RECOVER: &[u8] = b"mm/v1/recover";

/// The PRF salt is app-wide: during username-less login the user is not known yet.
/// This does not weaken secrecy: the PRF output is unique to each passkey.
pub fn prf_salt() -> [u8; 32] {
    Sha256::digest(b"mm/v1/prf-salt").into()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("could not get random bytes")]
    Random,
    #[error("encryption failed")]
    Encrypt,
    #[error("record is corrupted or was tampered with")]
    Decrypt,
    #[error("invalid data format")]
    Format,
    #[error("invalid recovery phrase")]
    BadPhrase,
    #[error("record is too large")]
    TooLarge,
    #[error("signature verification failed")]
    BadSignature,
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| CryptoError::Random)?;
    Ok(b)
}

/// A 256-bit secret, wiped when the value is dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop, PartialEq, Eq)]
pub struct Key32([u8; KEY_LEN]);

impl std::fmt::Debug for Key32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key32(***)")
    }
}

impl Key32 {
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self(random()?))
    }
    pub fn from_slice(b: &[u8]) -> Result<Self, CryptoError> {
        Ok(Self(b.try_into().map_err(|_| CryptoError::Format)?))
    }
    pub fn expose(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

pub type Dek = Key32;
pub type Kek = Key32;

fn hkdf(ikm: &[u8], info: &[u8]) -> Key32 {
    let mut okm = Zeroizing::new([0u8; KEY_LEN]);
    Hkdf::<Sha256>::new(None, ikm)
        .expand(info, okm.as_mut())
        .expect("32 bytes is valid for HKDF-SHA256");
    Key32(*okm)
}

/// KEK from the WebAuthn PRF output.
pub fn kek_from_prf(prf_output: &[u8]) -> Result<Kek, CryptoError> {
    if prf_output.len() < KEY_LEN {
        return Err(CryptoError::Format);
    }
    Ok(hkdf(prf_output, INFO_KEK_PRF))
}

/// A 12-word BIP39 recovery phrase.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Phrase {
    entropy: [u8; PHRASE_ENTROPY_LEN],
}

impl Phrase {
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self { entropy: random()? })
    }

    /// Case and extra spaces do not matter: the phrase is typed on a phone.
    pub fn parse(s: &str) -> Result<Self, CryptoError> {
        let normal = Zeroizing::new(s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase());
        let m = bip39::Mnemonic::parse_in(bip39::Language::English, normal.as_str())
            .map_err(|_| CryptoError::BadPhrase)?;
        let (bytes, len) = m.to_entropy_array();
        if len != PHRASE_ENTROPY_LEN {
            return Err(CryptoError::BadPhrase);
        }
        let mut entropy = [0u8; PHRASE_ENTROPY_LEN];
        entropy.copy_from_slice(&bytes[..len]);
        Ok(Self { entropy })
    }

    pub fn words(&self) -> Vec<String> {
        bip39::Mnemonic::from_entropy_in(bip39::Language::English, &self.entropy)
            .expect("16 bytes give 12 words")
            .words()
            .map(str::to_owned)
            .collect()
    }

    pub fn kek(&self) -> Kek {
        hkdf(&self.entropy, INFO_KEK_PHRASE)
    }

    /// The login key is independent of the KEK: they use different `info`, so
    /// the public key and signatures reveal nothing useful for decryption.
    pub fn auth_key(&self) -> PhraseAuthKey {
        let mut info = INFO_AUTH_PHRASE.to_vec();
        for ctr in 0u8.. {
            info.truncate(INFO_AUTH_PHRASE.len());
            info.push(ctr);
            let k = hkdf(&self.entropy, &info);
            // A value outside the curve order has a probability near 2^-32: try the next counter.
            if let Ok(sk) = SigningKey::from_slice(k.expose()) {
                return PhraseAuthKey(sk);
            }
        }
        unreachable!("the counter cannot run out in practice")
    }
}

pub struct PhraseAuthKey(SigningKey);

impl PhraseAuthKey {
    /// Compressed SEC1 point, 33 bytes.
    pub fn public(&self) -> Vec<u8> {
        self.0.verifying_key().to_encoded_point(true).as_bytes().to_vec()
    }
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        let sig: Signature = self.0.sign(msg);
        sig.to_bytes().to_vec()
    }
}

pub fn verify_phrase_signature(public: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
    let vk = VerifyingKey::from_sec1_bytes(public).map_err(|_| CryptoError::BadSignature)?;
    let sig = Signature::from_slice(sig).map_err(|_| CryptoError::BadSignature)?;
    vk.verify(msg, &sig).map_err(|_| CryptoError::BadSignature)
}

/// A record id derived from a name, so offline devices agree on it without creating
/// duplicates. It is keyed with the DEK because the server sees record ids: an unkeyed
/// hash would let it test names such as "biedronka" from a dictionary.
pub fn named_id(dek: &Dek, purpose: &[u8], name: &str) -> Uuid {
    let normalized = Zeroizing::new(crate::model::normalize_name(name));
    let mut info = purpose.to_vec();
    info.push(0);
    info.extend_from_slice(normalized.as_bytes());
    let mut okm = [0u8; 16];
    Hkdf::<Sha256>::new(Some(b"mm/v1/named-id"), dek.expose())
        .expand(&info, &mut okm)
        .expect("16 bytes is valid for HKDF-SHA256");
    info.zeroize();
    uuid::Builder::from_random_bytes(okm).into_uuid()
}

pub fn merchant_id(dek: &Dek, name: &str) -> Uuid {
    named_id(dek, b"merchant", name)
}

/// Message signed with the phrase key. The purpose is part of the message
/// so that a login signature cannot be reused to add a passkey.
pub fn phrase_challenge(purpose: &str, challenge_id: Uuid, nonce: &[u8]) -> Vec<u8> {
    let mut m = LABEL_RECOVER.to_vec();
    m.push(purpose.len() as u8);
    m.extend_from_slice(purpose.as_bytes());
    m.extend_from_slice(challenge_id.as_bytes());
    m.extend_from_slice(nonce);
    m
}

fn seal(key: &Key32, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    let nonce: [u8; NONCE_LEN] = random()?;
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
    out.push(BLOB_FORMAT);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn open(key: &Key32, blob: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.len() < 1 + NONCE_LEN || blob[0] != BLOB_FORMAT {
        return Err(CryptoError::Format);
    }
    let (nonce, ct) = blob[1..].split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Decrypt)
}

/// The wrap slot is part of the AAD: a copy cannot be moved between slots or accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapSlot {
    Prf,
    Phrase,
}

impl WrapSlot {
    fn tag(self) -> u8 {
        match self {
            Self::Prf => 1,
            Self::Phrase => 2,
        }
    }
}

fn wrap_aad(user_id: Uuid, slot: WrapSlot) -> Vec<u8> {
    let mut a = LABEL_WRAP.to_vec();
    a.extend_from_slice(user_id.as_bytes());
    a.push(slot.tag());
    a
}

pub fn wrap_dek(kek: &Kek, dek: &Dek, user_id: Uuid, slot: WrapSlot) -> Result<Vec<u8>, CryptoError> {
    seal(kek, dek.expose(), &wrap_aad(user_id, slot))
}

pub fn unwrap_dek(kek: &Kek, blob: &[u8], user_id: Uuid, slot: WrapSlot) -> Result<Dek, CryptoError> {
    let pt = open(kek, blob, &wrap_aad(user_id, slot))?;
    Key32::from_slice(&pt)
}

/// Everything a record's ciphertext is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordAad {
    pub user_id: Uuid,
    pub record_id: Uuid,
    pub schema: u32,
    /// Without the revision the server could silently return an old version of a record.
    pub revision: u64,
}

impl RecordAad {
    fn encode(&self) -> Vec<u8> {
        let mut a = LABEL_RECORD.to_vec();
        a.extend_from_slice(self.user_id.as_bytes());
        a.extend_from_slice(self.record_id.as_bytes());
        a.extend_from_slice(&self.schema.to_le_bytes());
        a.extend_from_slice(&self.revision.to_le_bytes());
        a
    }
}

const MIN_BUCKET: usize = 1024;
pub const MAX_BUCKET: usize = 1 << 20;

/// Record sizes are padded to powers of two, starting at 1 KiB.
pub fn pad(plain: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let need = plain.len().checked_add(4).ok_or(CryptoError::TooLarge)?;
    let bucket = need.next_power_of_two().max(MIN_BUCKET);
    if bucket > MAX_BUCKET {
        return Err(CryptoError::TooLarge);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(bucket));
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out.extend_from_slice(plain);
    out.resize(bucket, 0);
    Ok(out)
}

fn unpad(padded: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let head: [u8; 4] = padded.get(..4).ok_or(CryptoError::Format)?.try_into().unwrap();
    let len = u32::from_le_bytes(head) as usize;
    let body = padded.get(4..4usize.checked_add(len).ok_or(CryptoError::Format)?).ok_or(CryptoError::Format)?;
    Ok(Zeroizing::new(body.to_vec()))
}

pub fn seal_record(dek: &Dek, aad: RecordAad, plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    seal(dek, &pad(plain)?, &aad.encode())
}

pub fn open_record(dek: &Dek, aad: RecordAad, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    unpad(&open(dek, blob, &aad.encode())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aad() -> RecordAad {
        RecordAad { user_id: Uuid::new_v4(), record_id: Uuid::new_v4(), schema: 1, revision: 1 }
    }

    #[test]
    fn record_round_trip() {
        let dek = Dek::generate().unwrap();
        let a = aad();
        let b = seal_record(&dek, a, "bread 3,50 zł".as_bytes()).unwrap();
        assert_eq!(&*open_record(&dek, a, &b).unwrap(), "bread 3,50 zł".as_bytes());
    }

    #[test]
    fn any_aad_change_is_rejected() {
        let dek = Dek::generate().unwrap();
        let a = aad();
        let b = seal_record(&dek, a, b"x").unwrap();
        for bad in [
            RecordAad { user_id: Uuid::new_v4(), ..a },
            RecordAad { record_id: Uuid::new_v4(), ..a },
            RecordAad { schema: 2, ..a },
            RecordAad { revision: 2, ..a },
        ] {
            assert_eq!(open_record(&dek, bad, &b), Err(CryptoError::Decrypt));
        }
    }

    #[test]
    fn tampering_and_wrong_key_are_rejected() {
        let dek = Dek::generate().unwrap();
        let a = aad();
        let mut b = seal_record(&dek, a, b"x").unwrap();
        assert_eq!(open_record(&Dek::generate().unwrap(), a, &b), Err(CryptoError::Decrypt));
        let last = b.len() - 1;
        b[last] ^= 1;
        assert_eq!(open_record(&dek, a, &b), Err(CryptoError::Decrypt));
    }

    #[test]
    fn nonce_is_fresh_for_equal_input() {
        let dek = Dek::generate().unwrap();
        let a = aad();
        assert_ne!(seal_record(&dek, a, b"x").unwrap(), seal_record(&dek, a, b"x").unwrap());
    }

    #[test]
    fn size_reveals_only_the_bucket() {
        let dek = Dek::generate().unwrap();
        let a = aad();
        let short = seal_record(&dek, a, b"a").unwrap();
        let long = seal_record(&dek, a, &[7u8; 900]).unwrap();
        assert_eq!(short.len(), long.len());
        let bigger = seal_record(&dek, a, &[7u8; 1500]).unwrap();
        assert_eq!(bigger.len() - short.len(), 1024);
    }

    #[test]
    fn padding_boundaries() {
        for n in [0, 1, 1019, 1020, 1021, 2044, 2045] {
            let p = pad(&vec![1; n]).unwrap();
            assert!(p.len().is_power_of_two() && p.len() >= n + 4);
            assert_eq!(unpad(&p).unwrap().len(), n);
        }
        assert_eq!(pad(&vec![0; MAX_BUCKET]).unwrap_err(), CryptoError::TooLarge);
    }

    #[test]
    fn corrupt_padding_length_does_not_panic() {
        let mut bad = vec![0u8; 16];
        bad[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(unpad(&bad).is_err());
        assert!(unpad(&[1, 2]).is_err());
    }

    #[test]
    fn phrase_has_twelve_words_and_round_trips() {
        let p = Phrase::generate().unwrap();
        let words = p.words();
        assert_eq!(words.len(), 12);
        let q = Phrase::parse(&format!("  {} ", words.join("   ").to_uppercase())).unwrap();
        assert_eq!(p.kek(), q.kek());
    }

    #[test]
    fn phrase_rejects_bad_input() {
        assert!(Phrase::parse("abandon abandon").is_err());
        assert!(Phrase::parse("").is_err());
        // Bad BIP39 checksum
        assert!(Phrase::parse(&["abandon"; 12].join(" ")).is_err());
        // 24 words: a valid phrase, but not our format
        let w24 = format!("{} art", ["abandon"; 23].join(" "));
        assert_eq!(Phrase::parse(&w24).err(), Some(CryptoError::BadPhrase));
    }

    #[test]
    fn known_vector_keeps_derivation_stable() {
        // Changing this derivation would make user data unreadable.
        let p = Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap();
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(p.kek().expose()), "35c28009bd0d760c86f97b92b6e99784d66ad48ce23e498a1756cad7fba5f4b4");
        assert_eq!(hex(&p.auth_key().public()), "0364169cc46cccbca684cb658159639a5d255f719dd64ee3a257292360ae2600a4");
    }

    #[test]
    fn auth_key_is_independent_from_kek() {
        let p = Phrase::generate().unwrap();
        assert_ne!(&p.auth_key().public()[1..], p.kek().expose().as_slice());
        let msg = phrase_challenge("login", Uuid::new_v4(), b"n");
        let sig = p.auth_key().sign(&msg);
        verify_phrase_signature(&p.auth_key().public(), &msg, &sig).unwrap();
    }

    #[test]
    fn signature_is_bound_to_purpose_and_key() {
        let p = Phrase::generate().unwrap();
        let id = Uuid::new_v4();
        let sig = p.auth_key().sign(&phrase_challenge("login", id, b"n"));
        let other = phrase_challenge("add-passkey", id, b"n");
        assert!(verify_phrase_signature(&p.auth_key().public(), &other, &sig).is_err());
        let stranger = Phrase::generate().unwrap();
        let m = phrase_challenge("login", id, b"n");
        assert!(verify_phrase_signature(&stranger.auth_key().public(), &m, &sig).is_err());
    }

    #[test]
    fn dek_copies_unwrap_only_in_their_slot() {
        let user = Uuid::new_v4();
        let dek = Dek::generate().unwrap();
        let kek = kek_from_prf(&[9u8; 32]).unwrap();
        let w = wrap_dek(&kek, &dek, user, WrapSlot::Prf).unwrap();
        assert_eq!(unwrap_dek(&kek, &w, user, WrapSlot::Prf).unwrap(), dek);
        assert!(unwrap_dek(&kek, &w, user, WrapSlot::Phrase).is_err());
        assert!(unwrap_dek(&kek, &w, Uuid::new_v4(), WrapSlot::Prf).is_err());
    }

    #[test]
    fn prf_output_too_short_is_rejected() {
        assert!(kek_from_prf(&[1u8; 16]).is_err());
    }

    #[test]
    fn merchant_ids_agree_across_devices_but_depend_on_the_secret_key() {
        let (a, b) = (Dek::generate().unwrap(), Dek::generate().unwrap());
        assert_eq!(merchant_id(&a, "  ŻABKA  Express"), merchant_id(&a, "żabka express"));
        assert_ne!(merchant_id(&a, "żabka"), merchant_id(&b, "żabka"));
        assert_ne!(merchant_id(&a, "żabka"), merchant_id(&a, "lidl"));
    }

    #[test]
    fn debug_does_not_print_key() {
        assert_eq!(format!("{:?}", Dek::generate().unwrap()), "Key32(***)");
    }
}
