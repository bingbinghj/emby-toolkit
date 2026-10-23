// 115 protocol from p115cipher (ChenyangGao), GPLv3. These keys belong to the wire protocol.
use crate::error::{ApiError, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use num_bigint_dig::BigUint;
use std::sync::LazyLock;

static MODULUS: LazyLock<BigUint> = LazyLock::new(|| {
    BigUint::parse_bytes(b"94467199538421168685115018334776065898663751652520808966691769684389754194866868839785962914624862265689699980316658987338198288176273874160782292722912223482699621202960645813656296092078123617049558650961406540632832570073725203873545017737008711614000139573916153236215559489283800593547775766023112169091",10).unwrap()
});
const SHORT: &[u8] = &[0x8d, 0xa5, 0xa5, 0x8d];
const LONG: &[u8] = &[
    0x78, 0x06, 0xad, 0x4c, 0x33, 0x86, 0x5d, 0x18, 0x4c, 0x01, 0x3f, 0x46,
];
const TABLE: &[u8] = &[
    240, 229, 105, 174, 191, 220, 191, 138, 26, 69, 232, 190, 125, 166, 115, 184, 222, 143, 231,
    196, 69, 218, 134, 196, 155, 100, 139, 20, 106, 180, 241, 170, 56, 1, 53, 158, 38, 105, 44,
    134, 0, 107, 79, 165, 54, 52, 98, 166, 42, 150, 104, 24, 242, 74, 253, 189, 107, 151, 143, 77,
    143, 137, 19, 183, 108, 142, 147, 237, 14, 13, 72, 62, 215, 47, 136, 216, 254, 254, 126, 134,
    80, 149, 79, 209, 235, 131, 38, 52, 219, 102, 123, 156, 126, 157, 122, 129, 50, 234, 182, 51,
    222, 58, 169, 89, 52, 102, 59, 170, 186, 129, 96, 72, 185, 213, 129, 156, 248, 108, 132, 119,
    255, 84, 120, 38, 95, 190, 232, 30, 54, 159, 52, 128, 92, 69, 44, 155, 118, 213, 27, 143, 204,
    195, 184, 245,
];
fn xor(data: &[u8], key: &[u8]) -> Vec<u8> {
    let prefix = data.len() % 4;
    data.iter()
        .enumerate()
        .map(|(i, b)| {
            b ^ key[if i < prefix {
                i
            } else {
                (i - prefix) % key.len()
            }]
        })
        .collect()
}
pub fn encrypt(data: &[u8]) -> String {
    let mut data = xor(data, SHORT);
    data.reverse();
    let mut source = vec![0; 16];
    source.extend(xor(&data, LONG));
    let mut result = vec![];
    for chunk in source.chunks(117) {
        let mut block = vec![2; 128 - chunk.len() - 1];
        block[0] = 0;
        block.push(0);
        block.extend(chunk);
        let encoded = BigUint::from_bytes_be(&block)
            .modpow(&65537u32.into(), &MODULUS)
            .to_bytes_be();
        result.extend(vec![0; 128 - encoded.len()]);
        result.extend(encoded);
    }
    STANDARD.encode(result)
}
pub fn decrypt(data: &str) -> Result<Vec<u8>> {
    let source = STANDARD
        .decode(data)
        .map_err(|_| ApiError::bad("115 加密响应无效"))?;
    if source.is_empty() || source.len() % 128 != 0 {
        return Err(ApiError::bad("115 加密响应长度无效"));
    }
    let mut output: Vec<u8> = vec![];
    for chunk in source.chunks(128) {
        let block = BigUint::from_bytes_be(chunk)
            .modpow(&65537u32.into(), &MODULUS)
            .to_bytes_be();
        let end = block
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| ApiError::bad("115 加密响应填充无效"))?;
        output.extend(&block[end + 1..]);
    }
    if output.len() < 16 {
        return Err(ApiError::bad("115 加密响应缺少密钥"));
    }
    let key: Vec<u8> = (0..12)
        .map(|i| TABLE[132 - i * 12] ^ output[i].wrapping_add(TABLE[i * 12]))
        .collect();
    let mut plain = xor(&output[16..], &key);
    plain.reverse();
    Ok(xor(&plain, SHORT))
}
