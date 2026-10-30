"""Exports MobileSAM (Apache-2.0) to the two ONNX files Snap's Cut out uses.

python tools/cutout/export.py <MobileSAM-repo> <out-dir>

encoder: image [1,3,1024,1024] (normalized, padded) -> embeddings [1,256,64,64]
decoder: embeddings + points -> low_res_masks [1,4,256,256] + iou_predictions [1,4]
         (mask 0 = single-mask output, 1..3 = part / object / whole)
"""
import sys, warnings
import torch

repo, out = sys.argv[1], sys.argv[2]
sys.path.insert(0, repo)
warnings.filterwarnings("ignore")
from mobile_sam import sam_model_registry
from mobile_sam.utils.onnx import SamOnnxModel

sam = sam_model_registry["vit_t"](checkpoint=f"{repo}/weights/mobile_sam.pt").eval()


class Encoder(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, image):
        return self.m.image_encoder(image)


with torch.no_grad():
    torch.onnx.export(Encoder(sam), torch.randn(1, 3, 1024, 1024), f"{out}/snap_cutout_encoder.onnx",
                      input_names=["image"], output_names=["embeddings"], opset_version=17, dynamo=False)
    dec = SamOnnxModel(sam, return_single_mask=False)
    inputs = (
        torch.randn(1, 256, 64, 64),
        torch.randint(0, 1024, (1, 3, 2), dtype=torch.float),
        torch.tensor([[1, 0, -1]], dtype=torch.float),
        torch.zeros(1, 1, 256, 256),
        torch.tensor([0], dtype=torch.float),
        torch.tensor([256, 256], dtype=torch.float),
    )
    torch.onnx.export(dec, inputs, f"{out}/snap_cutout_decoder.onnx",
                      input_names=["image_embeddings", "point_coords", "point_labels", "mask_input", "has_mask_input", "orig_im_size"],
                      output_names=["masks", "iou_predictions", "low_res_masks"],
                      dynamic_axes={"point_coords": {1: "num_points"}, "point_labels": {1: "num_points"}},
                      opset_version=17, dynamo=False)
print("exported")
