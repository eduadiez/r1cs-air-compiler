pragma circom 2.2.3;
include "../../vendor/circomlib/sha256/sha256.circom";

// SHA-256 of a 256-bit message, with the 256 bits of the hash as public
// outputs. Large enough that only the packed layout fits the size limits.
component main = Sha256(256);
