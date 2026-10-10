PhotoCraft Generative Fill fork — UNSIGNED Windows x64 preview
https://github.com/kezerco/photocraft-generative

Extract every file in this archive into one folder, then DOUBLE-CLICK photocraft.exe. No batch file or command line is needed. Your first time using the feature: open an 8/16-bit RGB image, select a small area, right-click the selection and choose Generative Fill (or use Edit > Generative Fill). Paste your fal.ai API key into the dialog and check the paid-request confirmation before clicking OK. Get your own key at https://fal.ai/dashboard/keys and check model prices before editing.

The key is stored securely for your Windows account in Windows Credential Manager, not in PhotoCraft documents, its settings or the portable folder. Future sessions can launch photocraft.exe directly. Leave the key box blank to keep using your saved key; enter a new one to replace it. The FAL_KEY environment variable remains available for scripts and takes precedence over a saved key.

The default prompt is "Make it realistic". FLUX.1 Pro Fill accepts a true mask; FLUX.2 Pro and Gemini Pro use a selection guide and may not obey it. The result is an undoable layer; Edge Pixels controls its soft boundary. Try copies of images first. Each confirmed request may be billed even if it fails.

This is a community fork, not an official PhotoCraft release. The program is unsigned; Windows or your organization may block it. Do not bypass organizational policies. The cloud API receives the render crop, mask and prompt; never share your key or upload private renders without permission. 32-bit HDR and non-RGB input are not supported. See docs/generative-fill-fork.md in the source repository for limitations and build instructions.

portable.txt causes preferences/autosaves to stay next to this executable in PhotoCraftData. Your key remains in Windows Credential Manager, not in that folder. PhotoCraft is MIT OR Apache-2.0; bundled licenses are included.
