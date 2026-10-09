# Generative Fill (fork preview)

**Status:** experimental native-only integration; untested against a paid fal.ai account and not yet verified on Windows. It is not part of the upstream PhotoCraft release.

## What it does

Edit › Generative Fill… uses three fal.ai endpoints behind one account and key: **FLUX.1 Pro Fill** (actual image + mask), **FLUX.2 Pro Edit**, and **Gemini 3 Pro Image Edit** (Nano Banana Pro). For the latter two, a grayscale selection guide is sent as a second image and the returned image is resized if needed; the model may still change content inside the selected region unpredictably. Only selected pixels are composited back onto a new layer. Undo restores the document. The command is `edit.generativeFill` with `{"model":"flux1Fill|flux2Pro|geminiPro", "prompt":"repair the face while preserving the lighting", "confirmCharge":true}`. FLUX.2/Gemini additionally require `"acceptGuidedEdit":true`, acknowledging that neither endpoint takes an actual mask. **For strict mask-driven generation, use FLUX.1 Pro Fill.** Its dialog has **no live preview** because every request costs money. No upload happens just from opening the dialog.

Current limitations: native desktop/CLI only; 8/16-bit RGB documents; crop at most 2048 pixels per side and 4 MP; no larger-render tiling; model-guided edits are not guaranteed to honor selections; no verified ICC colour conversion for non-sRGB 8/16-bit images; no tested model output consistency. **32-bit float HDR/linear/EXR documents are refused** until colour management is implemented: export an sRGB PNG copy first. The crop is flattened to opaque 8-bit RGB for the API, while the *output layer* retains the document's original RGB depth and transparency outside the selection. **A 16-bit document remains 16-bit, but AI-generated detail comes back at the model's output precision, not true 16-bit fidelity.** Treat this as a preview, not a professional-ready renderer workflow.

## Windows trial

**You do not have to push to GitHub first.** Copy `dist/photocraft-generative-source.zip` from this working folder to your Windows PC. Install [Rust (MSVC)](https://rustup.rs/) and Visual Studio Build Tools with the **Desktop development with C++** workload, then run in PowerShell:

```powershell
Expand-Archive .\photocraft-generative-source.zip -DestinationPath "$env:USERPROFILE\photocraft-generative"
cd "$env:USERPROFILE\photocraft-generative"
cargo build --release --locked -p photocraft
Copy-Item .\packaging\windows\portable.txt .\target\release\portable.txt
cd .\target\release
```

The `Generative Fill Windows` GitHub Actions workflow can later produce an **unsigned portable build** after the feature branch is pushed. For now, test the local build on copies of your renders. An unsigned Windows app may be stopped by Windows security policies; don't bypass an organizational policy. Until a live API trial passes, do not use it on irreplaceable files.

Register an account at [fal.ai](https://fal.ai), add a small prepaid balance, then create an API key. In PowerShell, from the directory containing the unsigned executable:

```powershell
$env:FAL_KEY = [System.Net.NetworkCredential]::new('', (Read-Host 'fal.ai API key' -AsSecureString)).Password
.\photocraft.exe
Remove-Item Env:FAL_KEY
```

This environment variable belongs to the launched process; the fork does not store it in preferences or documents. The prompt avoids putting the key in your PowerShell history. Do not put keys in PhotoCraft command JSON, screenshots, GitHub issues, GitHub Actions, or committed files. Starting the app from Explorer won't inherit a key set only inside PowerShell.

1. Save a copy of your render, select a small region, then choose **Edit › Generative Fill…**.
2. Pick a model, type the prompt and tick **Agree to paid API request**; press OK. If you choose FLUX.2 or Gemini, a second checkbox explicitly warns that the selection is not an exact model mask. The dialog shows the crop dimensions and indicative pricing. Check the live provider prices for [masked FLUX.1 Fill](https://fal.ai/models/fal-ai/flux-pro/v1/fill), [FLUX.2 Pro Edit](https://fal.ai/models/fal-ai/flux-2-pro/edit) or [Gemini Pro Edit](https://fal.ai/models/fal-ai/nano-banana-pro/edit) before spending.
3. Compare the returned layer with the original. Disable/remove it if the face, texture or geometry is not correct. If the model rejects the image or returns another size, the command reports an error without changing the document, **but the provider may still charge**.

The request uses HTTPS to fal.ai and fetches the resulting image from a permitted HTTPS host. It is a cloud workflow: the cropped render, selection mask and prompt leave your computer. No account or key is needed for the rest of PhotoCraft.

## Maintaining the fork

The original repository is `upstream`; your fork is `origin`. On the feature branch, fetch and merge upstream updates before publishing a new build:

```sh
git fetch upstream origin
git switch feature/generative-fill-fal
git merge upstream/main
git push origin feature/generative-fill-fal
```

If upstream implements its own generative commands, resolve any conflicts deliberately instead of blindly discarding either implementation.
