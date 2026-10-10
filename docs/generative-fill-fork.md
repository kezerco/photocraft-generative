# Generative Fill (fork preview)

**Status:** experimental native-only integration, not part of the upstream PhotoCraft release. A user has successfully tested masked FLUX.1 Fill on Windows; FLUX.2 Pro and Gemini Pro have local validation tests but still need live editing/quality checks. Unsigned builds may be blocked by Windows security policies.

## What it does

Edit › Generative Fill… uses three fal.ai endpoints behind one account and key: **FLUX.1 Pro Fill** (actual image + mask), **FLUX.2 Pro Edit**, and **Gemini 3 Pro Image Edit** (Nano Banana Pro). For the latter two, a grayscale selection guide is sent as a second image and the returned image is resized if needed; the model may still change content inside the selected region unpredictably. The selected area is expanded by a small adjustable margin (12 pixels by default, 0–24) so features can extend naturally; the output edge is softly blended. Set `edgePixels:0` for exact clipping. Pixels well outside that margin are unchanged. Undo restores the document. The command is `edit.generativeFill` with `{"model":"flux1Fill|flux2Pro|geminiPro", "prompt":"repair the face while preserving the lighting", "confirmCharge":true}`. FLUX.2/Gemini additionally require `"acceptGuidedEdit":true`, acknowledging that neither endpoint takes an actual mask. **For strict mask-driven generation, use FLUX.1 Pro Fill.** The dialog starts with the prompt **“Make it realistic”**, which you can replace for unusual cases. It has **no live preview** because every request costs money. No upload happens just from opening the dialog.

Current limitations: native desktop/CLI only; 8/16-bit RGB documents; crop at most 2048 pixels per side and 4 MP; no larger-render tiling; model-guided edits are not guaranteed to honor selections; no verified ICC colour conversion for non-sRGB 8/16-bit images; no tested model output consistency. **32-bit float HDR/linear/EXR documents are refused** until colour management is implemented: export an sRGB PNG copy first. The crop is flattened to opaque 8-bit RGB for the API; masked FLUX.1 Fill pads a rectangular crop to a square, then resizes and removes the padding from a square model result before compositing. Different model output resolution alone is not an error; a masked output at either square or original-crop aspect is accepted. An output with an incompatible aspect ratio is rejected rather than stretched onto a face (the error gives both dimensions and fal.ai may still charge). The *output layer* retains the document's original RGB depth and transparency outside the selection margin. **A 16-bit document remains 16-bit, but AI-generated detail comes back at the model's output precision, not true 16-bit fidelity.** Treat this as a preview, not a professional-ready renderer workflow.

## Windows trial

**Easiest:** download the unsigned portable ZIP from [this fork's Releases](https://github.com/kezerco/photocraft-generative/releases), extract all files to a folder, then double-click **`START-PHOTOCRAFT.cmd`**. Paste your fal.ai API key *at the prompt* (nothing appears while you paste). Do not type it inside the launcher. Use the launcher each time; it passes the key to the app for that run without storing it in preferences. `portable.txt` keeps the fork's settings beside the app.

**Build from source instead:** install [Rust (MSVC)](https://rustup.rs/) and Visual Studio Build Tools with the **Desktop development with C++** workload, then run in PowerShell from the source checkout:

```powershell
cargo build --release --locked -p photocraft
Copy-Item .\packaging\windows\portable.txt .\target\release\portable.txt
Copy-Item .\packaging\windows\START-PHOTOCRAFT.cmd .\target\release\START-PHOTOCRAFT.cmd
cd .\target\release
```

The [Generative Fill Windows workflow](https://github.com/kezerco/photocraft-generative/actions/workflows/generative-windows.yml) creates unsigned Windows builds in Actions. Releases provide an easier download. Test on copies of your renders; don't bypass your organization's Windows security policies.

Register an account at [fal.ai](https://fal.ai), add a small balance, then create an API key. **When using the release ZIP, the launcher handles this**. If launching manually from PowerShell in the directory containing `photocraft.exe`, run:

```powershell
$env:FAL_KEY = [System.Net.NetworkCredential]::new('', (Read-Host 'fal.ai API key' -AsSecureString)).Password
.\photocraft.exe
Remove-Item Env:FAL_KEY
```

This environment variable belongs to the launched process; the fork does not store it in preferences or documents. The prompt avoids putting the key in your PowerShell history. Do not put keys in PhotoCraft command JSON, screenshots, GitHub issues, GitHub Actions, or committed files. Starting the app from Explorer won't inherit a key set only inside PowerShell.

1. Save a copy of your render, select a small region, then choose **Edit › Generative Fill…**.
2. Pick a model, adjust the prefilled “Make it realistic” prompt and edge margin if needed, then tick **Agree to paid API request**; press OK. If you choose FLUX.2 or Gemini, a second checkbox explicitly warns that the selection is not an exact model mask. The dialog shows the crop dimensions and indicative pricing. Check the live provider prices for [masked FLUX.1 Fill](https://fal.ai/models/fal-ai/flux-pro/v1/fill), [FLUX.2 Pro Edit](https://fal.ai/models/fal-ai/flux-2-pro/edit) or [Gemini Pro Edit](https://fal.ai/models/fal-ai/nano-banana-pro/edit) before spending.
3. Compare the returned layer with the original. Disable/remove it if the face, texture or geometry is not correct. If the model rejects the image or returns an incompatible aspect ratio, the command reports an error without changing the document, **but the provider may still charge**.

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
