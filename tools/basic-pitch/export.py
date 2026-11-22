"""Write Basic Pitch's weights (the ICASSP 2022 model, Apache 2.0, Spotify) in the flat layout
crates/analysis/src/poly/model.rs reads, from the ONNX file the basic-pitch package ships.

  python export.py crates/analysis/src/poly/basic_pitch.bin

Needs `pip install basic-pitch onnx`. The file is committed, so the build never runs this; run it
again only to check that the committed weights are the package's.

Layout, little-endian: the magic b"BPW1", then f32 arrays in this order (shapes as in the ONNX
graph, row-major): CQT kernel real part [36, 256], imaginary part [36, 256], the octave low-pass
[256], the CQT bin scale [309], the batch norm after the CQT [2] (scale, shift), then each
convolution's weights [out, in, time, freq] and bias [out], batch norms already folded in by the
ONNX export: onset 1 [32, 8, 5, 5], contour 1 [8, 8, 3, 39], contour 2 [1, 8, 5, 5],
note 1 [32, 1, 7, 7], note 2 [1, 32, 7, 3], onset 2 [1, 33, 3, 3].
"""

import os
import sys

import numpy as np
import onnx
from onnx import numpy_helper
from basic_pitch import ICASSP_2022_MODEL_PATH

path = os.path.join(os.path.dirname(str(ICASSP_2022_MODEL_PATH)), "nmp.onnx")
graph = onnx.load(path).graph
inits = {i.name: numpy_helper.to_array(i) for i in graph.initializer}


def conv(prefix):
    """Weights and bias of the one Conv node whose name starts with `prefix`."""
    nodes = [n for n in graph.node if n.op_type == "Conv" and n.name.startswith(prefix)]
    assert len(nodes) == 1, (prefix, [n.name for n in nodes])
    w, b = (inits[i] for i in nodes[0].input[1:3])
    return [w, b]


def const(name):
    return inits[name]


def cqt_kernels():
    """The real kernel feeds the CQT conv that is not negated; the imaginary one the negated one."""
    real = const("const_fold_opt__655").reshape(36, 256)
    imag = const("const_fold_opt__664").reshape(36, 256)
    return [real, imag]


bn_scale = [n for n in graph.node if n.name.startswith("model_1/batch_normalization/FusedBatchNormV3") and n.op_type == "Mul"]
bn_shift = [n for n in graph.node if n.name.startswith("model_1/batch_normalization/FusedBatchNormV3") and n.op_type == "Add"]
arrays = cqt_kernels() + [
    const("const_fold_opt__734").reshape(256),
    const("model_1/cq_t2010v2_1/Sqrt;model_1/cq_t2010v2_1/Sqrt").reshape(309),
    np.array([inits[bn_scale[0].input[1]][0], inits[bn_shift[0].input[1]][0]]),
]
arrays += conv("model_1/re_lu_3/Relu")  # onset 1
arrays += conv("model_1/re_lu_1/Relu")  # contour 1
arrays += conv("model_1/contours-reduced")  # contour 2
arrays += conv("model_1/re_lu_2/Relu")  # note 1
arrays += conv("model_1/conv2d_3")  # note 2
arrays += conv("model_1/conv2d_5")  # onset 2
shapes = [(36, 256), (36, 256), (256,), (309,), (2,), (32, 8, 5, 5), (32,), (8, 8, 3, 39), (8,), (1, 8, 5, 5), (1,),
          (32, 1, 7, 7), (32,), (1, 32, 7, 3), (1,), (1, 33, 3, 3), (1,)]
with open(sys.argv[1], "wb") as f:
    f.write(b"BPW1")
    for a, s in zip(arrays, shapes):
        assert a.shape == s, (a.shape, s)
        f.write(np.ascontiguousarray(a, dtype="<f4").tobytes())
print(sys.argv[1], sum(a.size for a in arrays), "floats")
