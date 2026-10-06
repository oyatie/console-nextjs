//! Proposed child tests for the real private browser_session crypto owner.
//! Test preparation only: not compiled, executed, or admitted.
//! No substitute implementation; missing seam is a build prerequisite, not RED.
use super::*;

const AAD_HEX: &str = "0000001d636f6e736f6c652f62726f777365722d73657373696f6e2d70726f6f6600000001000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f000000006b49d200";
const KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const NONCE_HEX: &str = "000102030405060708090a0b";
const PLAINTEXT: &str = "example.signed.proof";
const CIPHERTEXT_HEX: &str = "227ab776b589a735fe28f0e5d48d561df1b9e852";
const TAG_HEX: &str = "afbeaa6a75e8f86c0b15c192482ac59e";

fn hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "literal hex must have an even length");
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn binding() -> Binding {
    Binding {
        codec_version: 1,
        account: uuid::Uuid::parse_str("00010203-0405-0607-0809-0a0b0c0d0e0f").unwrap(),
        company: uuid::Uuid::parse_str("10111213-1415-1617-1819-1a1b1c1d1e1f").unwrap(),
        family: uuid::Uuid::parse_str("20212223-2425-2627-2829-2a2b2c2d2e2f").unwrap(),
        source: uuid::Uuid::parse_str("30313233-3435-3637-3839-3a3b3c3d3e3f").unwrap(),
        context: uuid::Uuid::parse_str("40414243-4445-4647-4849-4a4b4c4d4e4f").unwrap(),
        expires_unix_seconds: 1_800_000_000,
    }
}

fn key() -> Key {
    Key::from_bytes(&hex(KEY_HEX)).unwrap()
}

fn vector_proof() -> StoredProof {
    StoredProof {
        codec_version: 1,
        token_hash: vec![0xa5; 32],
        nonce: hex(NONCE_HEX),
        tag: hex(TAG_HEX),
        ciphertext: hex(CIPHERTEXT_HEX),
    }
}

fn no_secret_text(text: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert!(
        !text.contains(&hex),
        "diagnostics must exclude credential hex"
    );
    assert!(
        !text.contains(&format!("{bytes:?}")),
        "diagnostics must exclude credential bytes"
    );
    // Use the installed OpenSSL utility, without a new test dependency.
    let standard = openssl::base64::encode_block(bytes);
    let url = standard.replace('+', "-").replace('/', "_");
    for encoded in [
        standard.as_str(),
        standard.trim_end_matches('='),
        url.as_str(),
        url.trim_end_matches('='),
    ] {
        assert!(
            !text.contains(encoded),
            "diagnostics must exclude encoded credentials"
        );
    }
    if let Ok(value) = std::str::from_utf8(bytes) {
        if !value.is_empty() {
            assert!(
                !text.contains(value),
                "diagnostics must exclude plaintext credentials"
            );
        }
    }
}

#[test]
fn browser_proof_vectors_match_independent_aad_and_aes256_gcm_literals() {
    assert_eq!(VERSION, 1);
    assert_eq!(MAX_PROOF_BYTES, 1_048_576);
    let binding = binding();
    let aad_bytes = aad(&binding).unwrap();
    assert_eq!(aad_bytes.len(), 125);
    assert_eq!(aad_bytes.as_slice(), hex(AAD_HEX).as_slice());

    let nonce: [u8; 12] = hex(NONCE_HEX).try_into().unwrap();
    let (ciphertext, tag) =
        encrypt_bytes(&key(), &nonce, &aad_bytes, PLAINTEXT.as_bytes()).unwrap();
    assert!(
        ciphertext == hex(CIPHERTEXT_HEX),
        "ciphertext must match the independent literal"
    );
    assert!(
        tag.as_slice() == hex(TAG_HEX).as_slice(),
        "tag must match the independent literal"
    );
    let opened = open(&key(), &binding, &vector_proof()).unwrap();
    assert!(
        opened.as_str() == PLAINTEXT,
        "literal ciphertext must decrypt exactly"
    );

    // Separately exercise the production AES primitive with the standard
    // AES256-GCM zero-key/zero-IV/no-AAD vector, not a vector derived by it.
    let zero_key = Key::from_bytes(&[0_u8; 32]).unwrap();
    let (ciphertext, tag) = encrypt_bytes(&zero_key, &[0_u8; 12], &[], &[0_u8; 16]).unwrap();
    assert!(ciphertext == hex("cea7403d4d606b6e074ec5d3baf39d18"));
    assert!(tag.as_slice() == hex("d0d1c8a799996bf0265b98b5d48ab919").as_slice());

    // Signed i64, not unsigned or fractional time.
    let mut negative_exp = binding.clone();
    negative_exp.expires_unix_seconds = -1;
    assert_eq!(&aad(&negative_exp).unwrap()[117..125], &[0xff_u8; 8]);
}

#[test]
fn browser_proof_bounds_are_inclusive_and_count_utf8_bytes() {
    let key = key();
    let binding = binding();
    for length in [1, 1_048_575, 1_048_576] {
        let plaintext = "p".repeat(length);
        let proof = seal(&key, &binding, [0xa5; 32], &plaintext).unwrap();
        assert_eq!(proof.codec_version, 1);
        assert_eq!(proof.token_hash.len(), 32);
        assert_eq!(proof.nonce.len(), 12);
        assert_eq!(proof.tag.len(), 16);
        assert_eq!(proof.ciphertext.len(), length);
        let opened = open(&key, &binding, &proof).unwrap();
        assert!(
            opened.as_str() == plaintext,
            "round trip must preserve all admitted bytes"
        );
    }
    let at_limit = "é".repeat(524_288);
    assert_eq!(at_limit.len(), 1_048_576);
    assert!(seal(&key, &binding, [0xa5; 32], &at_limit).is_ok());
    let over_limit = format!("{at_limit}p");
    assert_eq!(over_limit.len(), 1_048_577);
    assert!(matches!(
        seal(&key, &binding, [0xa5; 32], &over_limit),
        Err(ProofError::Shape)
    ));
    assert!(matches!(
        seal(&key, &binding, [0xa5; 32], ""),
        Err(ProofError::Shape)
    ));
    assert!(matches!(
        seal(&key, &binding, [0xa5; 32], &"p".repeat(1_048_577)),
        Err(ProofError::Shape)
    ));

    let first = seal(&key, &binding, [0xa5; 32], PLAINTEXT).unwrap();
    let second = seal(&key, &binding, [0xa5; 32], PLAINTEXT).unwrap();
    assert!(
        first.nonce != second.nonce,
        "ordinary sealing must draw a fresh nonce"
    );
}

#[test]
fn browser_proof_tampering_and_each_authenticated_binding_fail_closed() {
    let key = key();
    let original = vector_proof();
    let binding = binding();
    for field in ["nonce", "tag", "ciphertext"] {
        let mut changed = original.clone();
        match field {
            "nonce" => changed.nonce[0] ^= 1,
            "tag" => changed.tag[0] ^= 1,
            "ciphertext" => changed.ciphertext[0] ^= 1,
            _ => unreachable!(),
        }
        assert!(matches!(
            open(&key, &binding, &changed),
            Err(ProofError::Authentication)
        ));
    }
    for field in [
        "account", "company", "family", "source", "context", "expiry",
    ] {
        let mut changed = binding.clone();
        let different = uuid::Uuid::from_bytes([0x7f; 16]);
        match field {
            "account" => changed.account = different,
            "company" => changed.company = different,
            "family" => changed.family = different,
            "source" => changed.source = different,
            "context" => changed.context = different,
            "expiry" => changed.expires_unix_seconds += 1,
            _ => unreachable!(),
        }
        assert!(matches!(
            open(&key, &changed, &original),
            Err(ProofError::Authentication)
        ));
    }
    let wrong_key = Key::from_bytes(&[0x7f; 32]).unwrap();
    assert!(matches!(
        open(&wrong_key, &binding, &original),
        Err(ProofError::Authentication)
    ));
    assert!(
        open(&key, &binding, &original).is_ok(),
        "tamper fixtures must leave the original untouched"
    );
}

#[test]
fn browser_proof_shapes_precede_crypto_and_debug_errors_redact_secrets() {
    let key = key();
    let binding = binding();
    for length in [0, 1, 31, 33] {
        assert!(matches!(
            Key::from_bytes(&vec![0x71; length]),
            Err(ProofError::Shape)
        ));
    }
    let mut wrong_version = binding.clone();
    wrong_version.codec_version = 2;
    assert!(matches!(aad(&wrong_version), Err(ProofError::Shape)));
    assert!(matches!(
        seal(&key, &wrong_version, [0xa5; 32], PLAINTEXT),
        Err(ProofError::Shape)
    ));
    assert!(matches!(
        open(&key, &wrong_version, &vector_proof()),
        Err(ProofError::Shape)
    ));

    for codec_version in [-1, 0, 2, i32::MAX] {
        let mut proof = vector_proof();
        proof.codec_version = codec_version;
        assert!(matches!(
            open(&key, &binding, &proof),
            Err(ProofError::Shape)
        ));
    }
    for (field, lengths) in [
        ("digest", vec![0, 31, 33]),
        ("nonce", vec![0, 11, 13]),
        ("tag", vec![0, 15, 17]),
        ("ciphertext", vec![0, 1_048_577]),
    ] {
        for length in lengths {
            let mut proof = vector_proof();
            // Also make authentication fail; Shape must win before crypto.
            proof.tag[0] ^= 1;
            match field {
                "digest" => proof.token_hash = vec![0x71; length],
                "nonce" => proof.nonce = vec![0x71; length],
                "tag" => proof.tag = vec![0x71; length],
                "ciphertext" => proof.ciphertext = vec![0x71; length],
                _ => unreachable!(),
            }
            let error = match open(&key, &binding, &proof) {
                Err(error) => error,
                Ok(_) => panic!("invalid proof shape must fail closed"),
            };
            assert!(matches!(&error, ProofError::Shape));
            for diagnostic in [format!("{error:?}"), format!("{error}")] {
                no_secret_text(&diagnostic, &proof.token_hash);
                no_secret_text(&diagnostic, &proof.nonce);
                no_secret_text(&diagnostic, &proof.tag);
                no_secret_text(&diagnostic, &proof.ciphertext);
                no_secret_text(&diagnostic, PLAINTEXT.as_bytes());
            }
        }
    }

    // Authenticated invalid UTF8 must fail before a String/JWT parser sees it.
    let nonce = [0x61_u8; 12];
    let (ciphertext, tag) = encrypt_bytes(&key, &nonce, &aad(&binding).unwrap(), &[0xff]).unwrap();
    let invalid_utf8 = StoredProof {
        codec_version: 1,
        token_hash: vec![0xa5; 32],
        nonce: nonce.to_vec(),
        tag: tag.to_vec(),
        ciphertext,
    };
    let error = match open(&key, &binding, &invalid_utf8) {
        Err(error) => error,
        Ok(_) => panic!("authenticated non-UTF8 proof must fail closed"),
    };
    assert!(matches!(&error, ProofError::Utf8));

    let proof = vector_proof();
    let opened = open(&key, &binding, &proof).unwrap();
    for diagnostic in [
        format!("{key:?}"),
        format!("{proof:?}"),
        format!("{opened:?}"),
        format!("{error:?}"),
        format!("{error}"),
    ] {
        for secret in [
            hex(KEY_HEX),
            proof.token_hash.clone(),
            proof.nonce.clone(),
            proof.tag.clone(),
            proof.ciphertext.clone(),
            PLAINTEXT.as_bytes().to_vec(),
        ] {
            no_secret_text(&diagnostic, &secret);
        }
    }
}
