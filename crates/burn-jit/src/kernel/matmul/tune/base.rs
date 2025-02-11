use burn_compute::tune::{AutotuneOperation, AutotuneOperationSet};
use burn_tensor::{Element, ElementConversion};

use crate::{
    compute::JitAutotuneKey,
    element::JitElement,
    kernel::{matmul::utils::init_matmul_output, prng::random_like_uniform},
    ops::numeric::empty_device,
    tensor::JitTensor,
    Runtime,
};

use super::key::MatmulAutotuneKey;

/// Set of matmul implementations available for autotune
/// Autotune key is given by concatenating the closest upper power of 2 of m, k and n
pub struct MatmulAutotuneOperationSet<R: Runtime, E: JitElement, const D: usize> {
    key: JitAutotuneKey,
    lhs: JitTensor<R, E, D>,
    rhs: JitTensor<R, E, D>,
    out: JitTensor<R, E, D>,
}
impl<R: Runtime, E: JitElement, const D: usize> MatmulAutotuneOperationSet<R, E, D> {
    fn new(lhs: JitTensor<R, E, D>, rhs: JitTensor<R, E, D>, out: JitTensor<R, E, D>) -> Self {
        Self {
            key: JitAutotuneKey::Matmul(MatmulAutotuneKey::new(&lhs.shape, &rhs.shape)),
            lhs,
            rhs,
            out,
        }
    }
}

impl<R: Runtime, E: JitElement + Element, const D: usize> AutotuneOperationSet<JitAutotuneKey>
    for MatmulAutotuneOperationSet<R, E, D>
{
    fn key(&self) -> JitAutotuneKey {
        self.key.clone()
    }

    fn autotunables(&self) -> Vec<Box<dyn AutotuneOperation>> {
        let random_bounds: (E, E) = ((-10.0).elem::<E>(), (10.0).elem::<E>());
        let lhs = random_like_uniform(&self.lhs, random_bounds.0, random_bounds.1);
        let rhs = random_like_uniform(&self.rhs, random_bounds.0, random_bounds.1);

        let out = empty_device(
            self.out.client.clone(),
            self.out.device.clone(),
            self.out.shape.clone(),
        );

        vec![
            Box::new(MemoryCoalescingMatmulDefault::new(
                lhs.clone(),
                rhs.clone(),
                out.clone(),
            )),
            Box::new(MemoryCoalescingMatmulW16x16::new(
                lhs.clone(),
                rhs.clone(),
                out.clone(),
            )),
            Box::new(Vec4TilingMatmulDefault::new(
                lhs.clone(),
                rhs.clone(),
                out.clone(),
            )),
            Box::new(Vec4TilingMatmulUnpaddedDefault::new(
                lhs.clone(),
                rhs.clone(),
                out.clone(),
            )),
        ]
    }

    fn fastest(self: Box<Self>, fastest_index: usize) -> Box<dyn AutotuneOperation> {
        match fastest_index {
            0 => Box::new(MemoryCoalescingMatmulDefault::new(
                self.lhs, self.rhs, self.out,
            )),
            1 => Box::new(MemoryCoalescingMatmulW16x16::new(
                self.lhs, self.rhs, self.out,
            )),
            2 => Box::new(Vec4TilingMatmulDefault::new(self.lhs, self.rhs, self.out)),
            3 => Box::new(Vec4TilingMatmulUnpaddedDefault::new(
                self.lhs, self.rhs, self.out,
            )),
            _ => panic!("Fastest index is out of bound"),
        }
    }
}

/// Executes autotune on matmul operations
pub fn matmul_autotune<R: Runtime, E: JitElement + Element, const D: usize>(
    lhs: JitTensor<R, E, D>,
    rhs: JitTensor<R, E, D>,
) -> JitTensor<R, E, D> {
    let client = lhs.client.clone();

    let output = init_matmul_output(&lhs, &rhs);

    let operation_set = Box::new(MatmulAutotuneOperationSet::new(lhs, rhs, output.clone()));

    client.autotune_execute(operation_set);

    output
}

macro_rules! matmul_tune_ops {
    ($name:ident, $func:expr) => {
        #[derive(new)]
        pub(crate) struct $name<R: Runtime, E: JitElement, const D: usize> {
            lhs: JitTensor<R, E, D>,
            rhs: JitTensor<R, E, D>,
            out: JitTensor<R, E, D>,
        }

        impl<R: Runtime, E: JitElement, const D: usize> AutotuneOperation for $name<R, E, D> {
            fn execute(self: Box<Self>) {
                #[allow(clippy::redundant_closure_call)]
                $func(self.lhs, self.rhs, self.out);
            }

            fn clone(&self) -> Box<dyn AutotuneOperation> {
                Box::new(Self {
                    lhs: self.lhs.clone(),
                    rhs: self.rhs.clone(),
                    out: self.out.clone(),
                })
            }
        }
    };
}

// Potentially better for small matrices.
matmul_tune_ops!(
    MemoryCoalescingMatmulDefault,
    crate::kernel::matmul::matmul_mem_coalescing_default
);

// Potentially better for small matrices.
matmul_tune_ops!(MemoryCoalescingMatmulW16x16, |lhs, rhs, out| {
    crate::kernel::matmul::matmul_mem_coalescing(lhs, rhs, out, 16, 16)
});

// Probably the fastest when fixed sizes.
matmul_tune_ops!(
    Vec4TilingMatmulDefault,
    crate::kernel::matmul::vec4::matmul_tiling_2d_vec4
);

// Probably the fastest otherwise.
matmul_tune_ops!(
    Vec4TilingMatmulUnpaddedDefault,
    crate::kernel::matmul::unpadded::matmul_tiling_2d_unpadded
);
