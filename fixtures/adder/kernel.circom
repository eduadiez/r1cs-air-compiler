pragma circom 2.2.3;
include "../../vendor/circomlib/binsum.circom";
include "../../vendor/circomlib/bitify.circom";
include "../../vendor/circomlib/comparators.circom";

// Adds three 32-bit words bitwise and checks the sum against a public value.
template Add3(n) {
    signal input word[3];
    signal input expected;
    signal output matches;
    component bits[3];
    component sum = BinSum(n, 3);
    for (var i = 0; i < 3; i++) {
        bits[i] = Num2Bits(n);
        bits[i].in <== word[i];
        for (var j = 0; j < n; j++) {
            sum.in[i][j] <== bits[i].out[j];
        }
    }
    component total = Bits2Num(n + 2);
    for (var j = 0; j < n + 2; j++) {
        total.in[j] <== sum.out[j];
    }
    component eq = IsEqual();
    eq.in[0] <== total.out;
    eq.in[1] <== expected;
    matches <== eq.out;
}

component main { public [expected] } = Add3(32);
