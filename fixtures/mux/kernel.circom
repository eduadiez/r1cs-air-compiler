pragma circom 2.2.3;
include "../../vendor/circomlib/multiplexer.circom";
include "../../vendor/circomlib/bitify.circom";

// Selects one of four public two-element rows by a 2-bit selector.
template Select() {
    signal input table[4][2];
    signal input sel;
    signal output out[2];
    component bits = Num2Bits(2);
    bits.in <== sel;
    component mux = Multiplexer(2, 4);
    mux.inp <== table;
    mux.sel <== sel;
    out <== mux.out;
}

component main { public [table] } = Select();
