//! Witness library for PILout produced by r1cs-air-compiler. Reads the stage-one
//! trace and public values projected by `air witness` from the directory named
//! by `R1CS_AIR_WITNESS`, and fills stage two from the artifact's own hints.
use std::sync::{Arc, RwLock};

use proofman_common::{
    trace::Trace, AirInstance, BufferPool, FromTrace, ProofCtx, ProofmanResult, SetupCtx,
};
use proofman_fields::{Goldilocks, PrimeField64};
use proofman_hints::{
    acc_mul_hint_fields, get_hint_ids_by_name, mul_hint_fields, HintFieldOptions,
};
use proofman_witness::{witness_library, WitnessComponent, WitnessLibrary, WitnessManager};

witness_library!(WitnessLib, Goldilocks);

fn words(name: &str) -> Vec<u64> {
    let dir = std::env::var("R1CS_AIR_WITNESS").expect("R1CS_AIR_WITNESS");
    let bytes = std::fs::read(format!("{dir}/{name}")).expect(name);
    assert_eq!(bytes.len() % 8, 0, "{name} is not a whole number of 8-byte words");
    bytes
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

struct Stage1<F> {
    rows: usize,
    cols: usize,
    values: Vec<F>,
}
impl<F: Send> Trace<F> for Stage1<F> {
    fn num_rows(&self) -> usize {
        self.rows
    }
    fn num_cols(&self) -> usize {
        self.cols
    }
    fn airgroup_id(&self) -> usize {
        0
    }
    fn air_id(&self) -> usize {
        0
    }
    fn commit_id(&self) -> Option<usize> {
        None
    }
    fn get_buffer(&mut self) -> Vec<F> {
        std::mem::take(&mut self.values)
    }
    fn is_shared_buffer(&self) -> bool {
        false
    }
    fn is_packed(&self) -> bool {
        false
    }
}

struct Kernel;

impl<F: PrimeField64> WitnessComponent<F> for Kernel {
    fn execute(
        &self,
        pctx: Arc<ProofCtx<F>>,
        _sctx: Arc<SetupCtx<F>>,
        ids: &RwLock<Vec<usize>>,
    ) -> ProofmanResult<()> {
        let publics = words("public.bin");
        assert_eq!(publics.len(), pctx.global_info.n_publics, "public value count");
        for (i, v) in publics.into_iter().enumerate() {
            pctx.set_public_value(v, i);
        }
        let id = pctx.add_instance(0, 0)?;
        ids.write().unwrap().push(id);
        Ok(())
    }

    fn calculate_witness(
        &self,
        stage: u32,
        pctx: Arc<ProofCtx<F>>,
        sctx: Arc<SetupCtx<F>>,
        instance_ids: &[usize],
        _n_cores: usize,
        _buffer_pool: &dyn BufferPool<F>,
    ) -> ProofmanResult<()> {
        let Some(&id) = instance_ids.first() else {
            return Ok(());
        };
        let setup = sctx.get_setup(0, 0)?;
        if stage == 1 {
            let cols = setup.stark_info.map_sections_n["cm1"] as usize;
            let rows = 1 << setup.stark_info.stark_struct.n_bits;
            let values: Vec<F> = words("trace.bin").into_iter().map(F::from_u64).collect();
            assert_eq!(values.len(), rows * cols, "trace words (rows × columns)");
            let mut trace = Stage1 { rows, cols, values };
            pctx.add_air_instance(AirInstance::new_from_trace(FromTrace::new(&mut trace)), id);
        } else if stage == 2 {
            let bin = setup.p_setup.p_expressions_bin;
            let im = get_hint_ids_by_name(bin, "im_col");
            if !im.is_empty() {
                let n = im.len();
                mul_hint_fields(
                    &sctx,
                    &pctx,
                    id,
                    n as u64,
                    im,
                    vec!["reference"; n],
                    vec!["numerator"; n],
                    vec![HintFieldOptions::default(); n],
                    vec!["denominator"; n],
                    vec![HintFieldOptions::inverse(); n],
                )?;
            }
            for (name, add) in [("gprod_col", false), ("gsum_col", true)] {
                for hint in get_hint_ids_by_name(bin, name) {
                    acc_mul_hint_fields(
                        &sctx,
                        &pctx,
                        id,
                        hint as usize,
                        "reference",
                        None,
                        "numerator_air",
                        "denominator_air",
                        HintFieldOptions::default(),
                        HintFieldOptions::inverse(),
                        add,
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl<F: PrimeField64> WitnessLibrary<F> for WitnessLib {
    fn register_witness(&mut self, wcm: &WitnessManager<F>) -> ProofmanResult<()> {
        wcm.register_component(Arc::new(Kernel));
        Ok(())
    }
}
