import numpy as np
import torch
from torch import nn

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
PIN_MEMORY = DEVICE == "cuda"


def prod(values):
    result = 1
    for v in values:
        result *= v
    return result


def print_param_count(module: nn.Module, ):
    param_count = sum(prod(p.shape) for p in module.parameters())
    print(f"Model has {param_count} parameters, which takes {param_count // 1024 / 1024:.3f} Mb")

    for name, child in module.named_children():
        child_param_count = sum(prod(p.shape) for p in child.parameters())
        print(f"  {name}: {child_param_count / param_count:.2f}")


def calc_gradient_norms(module: nn.Module):
    norms = []

    for param in module.parameters():
        if param.grad is not None:
            norms.append((param.grad.detach() ** 2).mean().item())

    return np.array(norms)


def calc_parameter_norm(module: nn.Module):
    return sum(param.detach().norm(p=2) for param in module.parameters()).item()


def guess_module_device(model: nn.Module) -> str:
    for p in model.parameters():
        return p.device
    return "cpu"


def scale_gradient(x: torch.tensor, t: float) -> torch.tensor:
    return t * x + (1 - t) * x.detach()


def inv_softmax(x, c):
    return torch.log(x) + c


def fake_quantize_scale(x, scale: float, bits: int):
    assert bits > 1, f"Need at least two bits (to properly represent min, max and 0), got {bits}"
    assert scale > 0.0

    a = 2 ** bits
    return torch.fake_quantize_per_tensor_affine(
        x,
        scale=2 * scale / (a - 2),
        zero_point=0,
        quant_min=-(a // 2 - 1),
        quant_max=(a // 2 - 1),
    )
