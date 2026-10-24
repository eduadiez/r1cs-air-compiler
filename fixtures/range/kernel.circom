pragma circom 2.2.3;
include "../../vendor/circomlib/comparators.circom";
include "../../vendor/circomlib/gates.circom";

// Checks lo <= x <= hi for 32-bit words; lo and hi are public.
template InRange(n) {
    signal input x;
    signal input lo;
    signal input hi;
    signal output ok;
    component low = GreaterEqThan(n);
    low.in[0] <== x;
    low.in[1] <== lo;
    component high = LessEqThan(n);
    high.in[0] <== x;
    high.in[1] <== hi;
    component both = AND();
    both.a <== low.out;
    both.b <== high.out;
    ok <== both.out;
}

component main { public [lo, hi] } = InRange(32);
