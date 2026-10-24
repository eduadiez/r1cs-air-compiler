pragma circom 2.2.3;

// Splits a value into 32 bits and checks that it is a 32-bit word.
template WordBits() {
    signal input value;
    signal output bit[32];
    var sum = 0;
    for (var i = 0; i < 32; i++) {
        bit[i] <-- (value >> i) & 1;
        bit[i] * (bit[i] - 1) === 0;
        sum += bit[i] * (2 ** i);
    }
    value === sum;
}

// Bitwise XOR of two 32-bit words: a XOR b = a + b - 2 * (a AND b), bit by bit.
template Xor() {
    signal input a[1];
    signal input b[1];
    signal output result[1];
    component aa = WordBits();
    component bb = WordBits();
    signal both[32];
    aa.value <== a[0];
    bb.value <== b[0];
    var sum = 0;
    for (var i = 0; i < 32; i++) {
        both[i] <== aa.bit[i] * bb.bit[i];
        sum += (aa.bit[i] + bb.bit[i] - 2 * both[i]) * (2 ** i);
    }
    result[0] <== sum;
}

component main { public [a, b] } = Xor();
