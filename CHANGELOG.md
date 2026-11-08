# Changelog

Every notable change to Hoarder. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [1.0.0](https://github.com/CPT-Dawn/Hoarder/releases/tag/v1.0.0) - 2026-11-08

### Features

- **e2e:** Headless Sway harness that paints by numbers ([6fa0052](https://github.com/CPT-Dawn/Hoarder/commit/6fa0052912cdd85d2b3a7d470551d8f99c51b204))
- **e2e:** Helper client hoards a Selection, then ghosts on cue ([09259f6](https://github.com/CPT-Dawn/Hoarder/commit/09259f6afe3a6b56ed7dd2a1b0b3fd5726b1e382))
- **e2e:** Poke the desktop with a virtual finger and keyboard ([a6a3630](https://github.com/CPT-Dawn/Hoarder/commit/a6a3630492910cca2b676e57867b15cd5d3f259e))
- **shot:** Freeze every output and spit All Outputs out as PNG ([0d63450](https://github.com/CPT-Dawn/Hoarder/commit/0d63450b2f96b739414f6a62e9ce1e0aa57369f2))
- **shot:** Stand rotated and flipped outputs upright, pixel for pixel ([5bc31fb](https://github.com/CPT-Dawn/Hoarder/commit/5bc31fbeb3a22dfe8cfe323391a50b8bfc4725b3))
- **clipboard:** Copy and paste that speak fluent wl-clipboard ([a918bfa](https://github.com/CPT-Dawn/Hoarder/commit/a918bfa594ecfb41936c70dd1531fc8492db79d0))
- **shot:** Captures land on the clipboard and outlive the shot ([c408b5a](https://github.com/CPT-Dawn/Hoarder/commit/c408b5add7551e4a29e8a681807b95855c74e4b4))
- **config:** A config that remembers your comments and your screenshots ([a0a392c](https://github.com/CPT-Dawn/Hoarder/commit/a0a392c826c45a69b5206540fc2b53a39f995a8e))
- **overlay:** Drag a Region out of the frozen screen ([51d3cbc](https://github.com/CPT-Dawn/Hoarder/commit/51d3cbc5189b810f37e26542e9f7ed9323c5ad6d))
- **overlay:** O, A, Enter, Space, arrows and Refine join the party ([440ec89](https://github.com/CPT-Dawn/Hoarder/commit/440ec892935da1b52ddefa32698843f3d77699b8))
- **shot:** Save in png, webp, jpeg, avif or qoi, your pick ([4661ef7](https://github.com/CPT-Dawn/Hoarder/commit/4661ef742b021d0677c34e02c0f94415eeb8dd5d))
- **save:** Encode the file on a worker so the clipboard never queues ([9175d78](https://github.com/CPT-Dawn/Hoarder/commit/9175d78a9352b450662ce3c8e6a20296e07151e9))
- **hook:** On_capture runs, reports, and never hogs the shot ([c101ccf](https://github.com/CPT-Dawn/Hoarder/commit/c101ccf10620ffc771f9c2e623ad51b913ddfe5b))
- **notify:** Screenshots now come with a thumbnail and buttons ([dc55e7a](https://github.com/CPT-Dawn/Hoarder/commit/dc55e7acafbaafa5ee18318f2bd4754c8e897922))
- **sound:** An optional click for the nostalgic shutterbug ([613ab07](https://github.com/CPT-Dawn/Hoarder/commit/613ab077fcf64a12c809483e698fa300db4c82a1))
- **flash:** Say cheese, briefly, and only after the freeze ([6ee0343](https://github.com/CPT-Dawn/Hoarder/commit/6ee03439d838d6a958ccd19a2847509c58463a51))
- **config:** [theme] gets a name and overrides, TOML stops arguing ([9249ea3](https://github.com/CPT-Dawn/Hoarder/commit/9249ea34e5e1e9fcfb8ebfb701fe202ccf72c95b))
- **theme:** The Overlay dresses like Zenith, on its own terms ([43587a1](https://github.com/CPT-Dawn/Hoarder/commit/43587a1e4330484f2174564f153d8bec442234fd))
- **daemon:** Copies outlive their apps, the Daemon stays polite ([2606a16](https://github.com/CPT-Dawn/Hoarder/commit/2606a16a0a5a48a2771d4e1399d15899fc59c958))
- **shot:** --live selects over the real desktop, then ducks out of frame ([a367d33](https://github.com/CPT-Dawn/Hoarder/commit/a367d33d61c7663f4a7e8d05ea33e9a5318efc70))
- **shot:** --delay counts down on a ring, then freezes at zero ([35b5b85](https://github.com/CPT-Dawn/Hoarder/commit/35b5b85a8afaf6381649f349e2d4e4c3c81200f0))
- **capture:** Ask each output if it speaks HDR before badging it ([4b11b55](https://github.com/CPT-Dawn/Hoarder/commit/4b11b55d985a8bd558ab24cba4823827ba4b4f05))
- **shot:** Mixed-scale and depth policies, upscale, clamp or split ([31b5789](https://github.com/CPT-Dawn/Hoarder/commit/31b5789e6f6a9a05e2f2c9a4423908d0c4e99743))
- **overlay:** Click a window, get exactly the window ([3b9b5a1](https://github.com/CPT-Dawn/Hoarder/commit/3b9b5a19784d6033f9a197b1c89fa27e46e0e417))
- **overlay:** Hover tints the window, W digs down the stack ([a1e4e4e](https://github.com/CPT-Dawn/Hoarder/commit/a1e4e4ef578fde4011290ba9b4b68a71e7a12776))
- **setup:** Keybindings in their own file, main config untouched ([a6fb49d](https://github.com/CPT-Dawn/Hoarder/commit/a6fb49d85e69c55ccdfa59cc1d5e68cf2a38020d))
- **doctor:** A house call for compositor, service and clipboard rivals ([0995075](https://github.com/CPT-Dawn/Hoarder/commit/0995075074bb8ba4f1de5be19fdb89ece3aada36))
- **cli:** Bare hoarder says hi and offers to start the Daemon ([7a21d57](https://github.com/CPT-Dawn/Hoarder/commit/7a21d57c534765c6fd996dd0825791c63a3c7f0e))
- **capture:** Freeze the cursor in its own little session ([cc0ba0d](https://github.com/CPT-Dawn/Hoarder/commit/cc0ba0d4c2f014acbeffe1cd90ea072dc4679ac4))
- **overlay:** A Loupe, a shy hint bar and a cursor you can toggle ([cd44c93](https://github.com/CPT-Dawn/Hoarder/commit/cd44c93e0bbf7af3b611952aaf81a7b8569e71da))
- **history:** Record every Normal copy as a Clip, dedup included ([58be391](https://github.com/CPT-Dawn/Hoarder/commit/58be391483e1d2c8db909cbe355ca14c4358fcfa))
- **history:** Restore, get, delete, blobs and a 0700 hoard ([7f8e4df](https://github.com/CPT-Dawn/Hoarder/commit/7f8e4df044059acc9106752dc00457affe43da3b))
- **shot:** Capture windows whole, even the shy covered ones ([4fb8824](https://github.com/CPT-Dawn/Hoarder/commit/4fb8824b20fd6f51aa5b19c94bbeb063b3a81d6f))
- **wayland:** Ask Hyprland which toplevel lives at which address ([e788c35](https://github.com/CPT-Dawn/Hoarder/commit/e788c35a45525d76930ddf5f3695f49c65cc242b))
- **ipc:** Copy and Captures move in with the Daemon, no fork left behind ([ae92d84](https://github.com/CPT-Dawn/Hoarder/commit/ae92d840594b0c42bafb6e6539eb15b678eae1c8))
- **compat:** Answer to wl-copy and wl-paste, accent included ([b368a61](https://github.com/CPT-Dawn/Hoarder/commit/b368a61b3a4b93495a27b6abc084cf1bc910aebd))
- **paste:** --watch tunes in to the Daemon instead of its own radio ([4374f62](https://github.com/CPT-Dawn/Hoarder/commit/4374f62592b063a57f52f4de480e5f1470bc67ee))
- **store:** Evict the oldest unpinned, spare the pinned favourites ([5fbc5f1](https://github.com/CPT-Dawn/Hoarder/commit/5fbc5f19a7845dbf1938e05229ed4210e372ba03))
- **daemon:** Pins, history clear, pause and resume for shy moments ([a77b0fb](https://github.com/CPT-Dawn/Hoarder/commit/a77b0fb0596e547b4ac1ae9ad58a139104ad353d))
- **pick:** Hoarder pick, a colour picker with a loupe on a leash ([7d6267c](https://github.com/CPT-Dawn/Hoarder/commit/7d6267c2e65f7e6449cf6495af8dd159de1b36ff))
- **privacy:** Sort Selections into Secret, Concealed and Normal ([1811b06](https://github.com/CPT-Dawn/Hoarder/commit/1811b063684af86ee56f1a9438531900ff882fab))
- **history:** Search op digs past the preview for long text ([76814f6](https://github.com/CPT-Dawn/Hoarder/commit/76814f6e840ee082e2220208402dc2ec39cd6a76))
- **picker:** Hoarder picker, a panel that finds what you hoarded ([a04b090](https://github.com/CPT-Dawn/Hoarder/commit/a04b090770857cd6c0132135d243630c38ee85a0))
- **ipc:** Clip events, state events and reload, all on the record ([817a9af](https://github.com/CPT-Dawn/Hoarder/commit/817a9afccfa56b9d00308d1e7c8086113d54bfc5))
- **picker:** Restore, then paste it for you (Ctrl+Shift+V for terms) ([772b147](https://github.com/CPT-Dawn/Hoarder/commit/772b14747b312a8390ca3053df7264eba949dc27))
- **store:** Seal History with age, keep search in memory only ([60566d3](https://github.com/CPT-Dawn/Hoarder/commit/60566d3dbf6f7036b6ebb9ec7d33159111790010))
- **daemon:** Keep the History key in the keyring, migrate on toggle ([8351b86](https://github.com/CPT-Dawn/Hoarder/commit/8351b8618ad14a582db10fa5d7426e4fb73a8cfe))
- **store:** Seal thumbnails and search text, merge #76 and #78 ([2b8cc0c](https://github.com/CPT-Dawn/Hoarder/commit/2b8cc0cd3d4a1073096f6f5e68d740ab08028e17))
- **picker:** Img:, txt:, file:, pin: and app: sift the hoard ([3e36618](https://github.com/CPT-Dawn/Hoarder/commit/3e366186e47ceba0e2c4e4daa7fcd496ff28a1a7))
- **picker:** Shift+Enter, Ctrl+P, Del and Ctrl+1-9 at your fingertips ([9306ce3](https://github.com/CPT-Dawn/Hoarder/commit/9306ce3acac3dca493483998a10ebaa57089b862))
- **picker:** Concealed clips wear dots until you hold Alt ([8aa7787](https://github.com/CPT-Dawn/Hoarder/commit/8aa7787a3e68d8ee1f0e15c1265522a10b0dd423))
- **picker:** A preview pane that shows the whole hoard, pixel-true ([0621ce6](https://github.com/CPT-Dawn/Hoarder/commit/0621ce6bd29790c368eac1a13284e1823a4c99da))
- **cli:** Completions, man pages and a dist kit for packagers ([d5a4741](https://github.com/CPT-Dawn/Hoarder/commit/d5a4741a4cea18b04d8224a369912c64728e4d46))
- **shot:** Output and last Targets, --confirm, and freeze=false obeyed ([5f05a85](https://github.com/CPT-Dawn/Hoarder/commit/5f05a85cb4bcc08dbb84d956c0e752693cb8e5c4))
- **config:** [**breaking**] Mixed_scale and depth move to [capture] ([7a83dee](https://github.com/CPT-Dawn/Hoarder/commit/7a83dee6172ee23d67a704d9e5ee0a5c17380f54))
- **store:** Files copied the GNOME way are files, not other ([1000768](https://github.com/CPT-Dawn/Hoarder/commit/10007688886cef1e03005fb71a03cce56306aa6c))
- **daemon:** Clipboard.on_clip runs a hook per copy, sans watcher ([5abbc7b](https://github.com/CPT-Dawn/Hoarder/commit/5abbc7b53db09f73407a9a7cd98afbba216ec6dc))
- **daemon:** Learn the focused app from Sway and Hyprland IPC too ([7496b42](https://github.com/CPT-Dawn/Hoarder/commit/7496b42b93d16c98594b43d781c4e2f305bc1460))
- **clipboard:** Copy and paste go wlr when ext data control is away ([d0e4960](https://github.com/CPT-Dawn/Hoarder/commit/d0e496089f9570be032357cd3d27a55e1355df7c))
- **overlay:** Fade in over 100 ms instead of slamming the lights off ([c8de541](https://github.com/CPT-Dawn/Hoarder/commit/c8de5411a598b37b4b72d4650064ba8cf7d8f5b7))
- **overlay:** Size the chrome by wp_fractional_scale_v1's say-so ([fe5dd31](https://github.com/CPT-Dawn/Hoarder/commit/fe5dd31a9ea76735c0d03d44bc736a985d9420c3))
- **picker:** Shift+arrows pan an image too big for the preview ([f9b6218](https://github.com/CPT-Dawn/Hoarder/commit/f9b621826e1dcce469d966fbc5a168e261ab02a8))
- **overlay:** Window labels wear their app's icon ([8e48be4](https://github.com/CPT-Dawn/Hoarder/commit/8e48be4f1a86d16906961b376af69748f21793e5))
- **overlay:** Refine grows a toolbar you can Tab through ([30fab6a](https://github.com/CPT-Dawn/Hoarder/commit/30fab6ad40b9cba8bdc4eae65350ad7bc75a3692))

### Fixes

- **config:** Let in-place saves settle before reloading ([7c98fb6](https://github.com/CPT-Dawn/Hoarder/commit/7c98fb6afd794f01378a8708441c116d4ddd961b))
- **daemon:** Stop strangling big copies with a non-blocking pipe ([6dc6910](https://github.com/CPT-Dawn/Hoarder/commit/6dc6910f44c776212d33170528f33fd49a92a01b))
- **config:** Keep the last good config when a save is caught mid-write ([95f82bb](https://github.com/CPT-Dawn/Hoarder/commit/95f82bbe0520b0a795c1105e7fc05dac41c9f7c9))
- **aur:** Keep makepkg's LTO away from the bundled C, link libgcc ([1c155ed](https://github.com/CPT-Dawn/Hoarder/commit/1c155ed302349595090fda5b7b64bfe0feb68232))
- **daemon:** Restoring over a Concealed Clip now wipes it for real ([221fdd2](https://github.com/CPT-Dawn/Hoarder/commit/221fdd2edb4cd4e1a691d17f3fc52a28832f4428))
- **setup:** Speak Lua to Hyprland and wait for the Daemon's hello ([b754a1e](https://github.com/CPT-Dawn/Hoarder/commit/b754a1e38cc5c3604ada23351405469d38949148))
- **daemon:** Keep the last good config through a half-saved file ([e3ab0d7](https://github.com/CPT-Dawn/Hoarder/commit/e3ab0d7d2e674174444f379db9ce10bcb9ac38ce))

### Performance

- **overlay:** Paint a fresh chrome buffer once, not twice ([18e639e](https://github.com/CPT-Dawn/Hoarder/commit/18e639eb3afb049a90db4f56fea03441cc94b672))
- **overlay:** Idle dimming is one pixel, stretched by the compositor ([d54dbc2](https://github.com/CPT-Dawn/Hoarder/commit/d54dbc2bb893002eec6fb1603069dfb680283f01))

### Refactoring

- **countdown:** Wear the Overlay's Options, ready for its theme ([fb3f881](https://github.com/CPT-Dawn/Hoarder/commit/fb3f88185765560316863bd7c54c25e2f1deab66))
- **chrome:** Tint follows whatever accent the theme hands it ([7cee4d9](https://github.com/CPT-Dawn/Hoarder/commit/7cee4d95eb3d7e9a7dc0b53f5fb5f04069fe62a4))
- **daemon:** Answer a ReplyTo, not a loose (client, id) pair ([d02d255](https://github.com/CPT-Dawn/Hoarder/commit/d02d255564eaa4dcba41ec8d9f9a5c141ceb23a0))
- A Clip's kind is a type, not a string with ambitions ([77bd599](https://github.com/CPT-Dawn/Hoarder/commit/77bd5995c0e2f53535bc1676f07ab02a807c2b1a))
- **config:** One write_atomic for every file we rewrite ([421b1c4](https://github.com/CPT-Dawn/Hoarder/commit/421b1c4d73297b1cf8b74f566e1b34fee16dcd4f))
- **ui:** One layer-shell bootstrap, and the flash moves in ([c207212](https://github.com/CPT-Dawn/Hoarder/commit/c207212d34c7862de7ca21a1489b7487aadb7519))
- **config:** Zenith's config.toml is read by hoarder-config ([4c1ad53](https://github.com/CPT-Dawn/Hoarder/commit/4c1ad5392932d691cef4bd32992a6605ca76c14d))
- **paste:** Sleep on the socket, not in 10 ms naps ([0bf6801](https://github.com/CPT-Dawn/Hoarder/commit/0bf6801e4c763cda201d1f4daa5e4750565fca16))

### Documentation

- Hoard research, glossary and first ADRs before any code ([4966c38](https://github.com/CPT-Dawn/Hoarder/commit/4966c385f61563d9c50664ed5d2307366c287e95))
- **adr:** Freeze the frame, not the pixels; publish the socket ([b20f7a5](https://github.com/CPT-Dawn/Hoarder/commit/b20f7a5734389eef91bf7a7b380b53357db28618))
- **adr:** Takeover, secrets tiers, DIY UI and a firm list of nopes ([a7985de](https://github.com/CPT-Dawn/Hoarder/commit/a7985de06dc787b857420068084cffaa122742b4))
- **plan:** The whole hoard, mapped from freeze to AUR ([2d2af26](https://github.com/CPT-Dawn/Hoarder/commit/2d2af2603be767c6ba85439b9f90e347f0f8fa77))
- **plan:** One seam to test them all (plus three small ones) ([a4ed708](https://github.com/CPT-Dawn/Hoarder/commit/a4ed708c18cef625c175a13263f30862a89c5f35))
- **plan:** Point milestones at the sliced tickets ([63056fd](https://github.com/CPT-Dawn/Hoarder/commit/63056fdb200e38eef6343f10bbdce18ba8906b30))
- **e2e:** Warn that Sway's 90 is everyone else's 270 ([f7ec8f0](https://github.com/CPT-Dawn/Hoarder/commit/f7ec8f0b6e2e91106e44dfefcbd3c756e394fd92))
- **testing:** How to fake a slow app and a systemd, for science ([1380ecd](https://github.com/CPT-Dawn/Hoarder/commit/1380ecd41f17ded822ca3495fb187bbd9ca0a1ec))
- **plan:** List paste and the selection event among the ops ([9c07808](https://github.com/CPT-Dawn/Hoarder/commit/9c07808b9343a17c7934e8ce8c5da9709a86291e))
- **ipc:** Write the socket contract down, and pack the crate's bags ([090e295](https://github.com/CPT-Dawn/Hoarder/commit/090e295b0c2e795dfe05549c3a8e4bc85775256e))
- **plan:** Spell out what encryption seals and what it shrugs at ([40524b3](https://github.com/CPT-Dawn/Hoarder/commit/40524b3b8e87e4cea2df5c8d8927cc927127fa96))
- **release:** The button, the checklist and the 20% rule ([3abbb68](https://github.com/CPT-Dawn/Hoarder/commit/3abbb6876147a1744b612fded9279b084ad0b510))
- **adr:** Concealed memory is locked without a single unsafe block ([35ce491](https://github.com/CPT-Dawn/Hoarder/commit/35ce491d5cada66339654d0ff4e11cab3735077b))
- Focus sources, file kinds and the Refine toolbar, written down ([43f7b9a](https://github.com/CPT-Dawn/Hoarder/commit/43f7b9a0c6888674a92adecd6c96b5623f042813))
- **setup:** Tell Lua Hyprland users where hoarder.lua goes ([722890e](https://github.com/CPT-Dawn/Hoarder/commit/722890e5cd9e8855e197baea9fc6f36fc75643f5))
- **config:** Document every key, and a test that keeps it so ([d4b3af8](https://github.com/CPT-Dawn/Hoarder/commit/d4b3af8d91518165d9eb6a93b7e0886b506bb280))
- **readme:** From work in progress to a proper front door ([8c5e3d1](https://github.com/CPT-Dawn/Hoarder/commit/8c5e3d12b753698801411fcd78a9b72758cf9fab))
- Add CONTRIBUTING and issue forms that ask for hoarder doctor ([b317a8c](https://github.com/CPT-Dawn/Hoarder/commit/b317a8c26c3e322d17ea46239d2986467cf1c680))

### Tests

- **shot:** Prove 10-bit outputs keep all 30 bits in a 16-bit PNG ([b21e975](https://github.com/CPT-Dawn/Hoarder/commit/b21e975ff0bd0976a116e46a6a4b438cf8520b55))
- **shot:** Cover screencopy fallback, KWin-style exit 3 and multi-output ([5d48388](https://github.com/CPT-Dawn/Hoarder/commit/5d483889bcc7528ffe278607d9ae391e225f15fd))
- **shot:** Hold 4K shot-to-clipboard to 150 ms, stopwatch in release ([4f33569](https://github.com/CPT-Dawn/Hoarder/commit/4f335696b027566eb4449e27b4e2d3dcaf4d7032))
- **clipboard:** No data control, no dice, exit 3 ([0cb6906](https://github.com/CPT-Dawn/Hoarder/commit/0cb6906b854c8fdf35b9a5ce64bf7718f6d15146))
- **overlay:** Cross-monitor drags, cancels and a 1:1 eyeball check ([b6c1cd2](https://github.com/CPT-Dawn/Hoarder/commit/b6c1cd28449ce2c9307f7d1b1ae5a93ef5fc04e2))
- **e2e:** Drain hoarder's pipes so fat PNGs stop clogging them ([226f264](https://github.com/CPT-Dawn/Hoarder/commit/226f264faa4a06233a0c6d3724686885dd2a2661))
- **config:** The default file grew a [theme], so save moved down ([1791dfe](https://github.com/CPT-Dawn/Hoarder/commit/1791dfe3ec0b0b223d7d4d04535a030ceb77a5c2))
- **notify:** Badges reach the bubble, and split files each get one ([d57d484](https://github.com/CPT-Dawn/Hoarder/commit/d57d484e97f958d87dc0710563ffe0b8ed41e8c0))
- **windows:** Sway and Hyprland replies, parsed from the fixtures ([6553933](https://github.com/CPT-Dawn/Hoarder/commit/6553933ea3ac99c919c04ae9b97a860f7a84ef25))
- **budget:** One scale to rule CI's slower clocks ([e87479d](https://github.com/CPT-Dawn/Hoarder/commit/e87479dcab5ee2deae1031f6e7e787299e01e96a))
- **clipboard:** Stop counting half-written lines as done ([946d234](https://github.com/CPT-Dawn/Hoarder/commit/946d2346348e573f9aa820f4a451ff6e8b7d8026))
- **bus:** Call Structure by its new name before clippy does ([ba78492](https://github.com/CPT-Dawn/Hoarder/commit/ba78492210565269a29c730a7f83b140859c9919))
- **privacy:** Swap a too-real AWS key id for a proper EXAMPLE ([6589479](https://github.com/CPT-Dawn/Hoarder/commit/6589479a24bf8024210e2fffa52e5220457cc2b0))
- **store:** Prove a v1 History grows its clips_text without tears ([7263860](https://github.com/CPT-Dawn/Hoarder/commit/72638606c53d838ae28bfecda0e1bb9b3f4e0083))
- **e2e:** Give the harness text fields that actually paste ([1c93f9c](https://github.com/CPT-Dawn/Hoarder/commit/1c93f9cbd2f3694d639301600ef8d5595c7b66e0))
- **picker:** Wait for the key to land before judging the preview ([8ef7b1f](https://github.com/CPT-Dawn/Hoarder/commit/8ef7b1f52e9901d661a56802d61eba3c875d8151))
- **budgets:** Weigh the idle Daemon with 1,000 Clips in History ([f6d81f8](https://github.com/CPT-Dawn/Hoarder/commit/f6d81f8e79b8f24fc088bc1af2bcdc948d54f2b0))
- **delay:** Let the Overlay finish fading in before judging its dim ([b9dae8f](https://github.com/CPT-Dawn/Hoarder/commit/b9dae8f4e362b183d01680ca82014b0439fd05db))
- **overlay:** Give grim a second chance in the Overlay helper ([af4216f](https://github.com/CPT-Dawn/Hoarder/commit/af4216fd7307ddc909e488666e76b739fab51016))
- **daemon:** Let History settle before counting idle wakeups ([2671e30](https://github.com/CPT-Dawn/Hoarder/commit/2671e30503ea6de028a8333974d8729621afbd34))
- **ipc:** Catch live reload trading a half-saved config for defaults ([05b3851](https://github.com/CPT-Dawn/Hoarder/commit/05b385102a4304dc24fa43f51ea470d74adfb617))
- **budgets:** Track the Daemon's memory instead of capping it ([dd550b6](https://github.com/CPT-Dawn/Hoarder/commit/dd550b6bd33857b0f1b2d81bc3a2e697f1a3832f))
- **dist:** Let service enable fail honestly without a Daemon ([6e643c0](https://github.com/CPT-Dawn/Hoarder/commit/6e643c0fe9617703e1bc1acd2d911669d5231693))

### Build and CI

- Fmt, clippy, MSRV, deny and a Sway that never sees daylight ([27d9f07](https://github.com/CPT-Dawn/Hoarder/commit/27d9f075a107fe470c6d53c7340884c984eba75b))
- Strip sway's realtime ambitions inside the container ([4310f8f](https://github.com/CPT-Dawn/Hoarder/commit/4310f8f832208c58b9b44e5682c56f51afbc33df))
- **deps:** Give wl-copy the xdg-mime it sniffs PNGs with ([7e08030](https://github.com/CPT-Dawn/Hoarder/commit/7e08030920133c7d649fbe7cad27abe871d8d8f5))
- **deps:** Hand Ubuntu jobs the xkbcommon that sctk builds against ([25024e1](https://github.com/CPT-Dawn/Hoarder/commit/25024e17c9c956e9adf54debf738b9e6a8dc8366))
- Let fix/** branches prove themselves too ([dbe7673](https://github.com/CPT-Dawn/Hoarder/commit/dbe7673df581c303e6083904b6338feb9bb2a94d))
- **deps:** Pin age 0.11 and excuse its build-time macro fossils ([5497a53](https://github.com/CPT-Dawn/Hoarder/commit/5497a53a6d66818179f5fcc32fa7ae97c1abbc40))
- **release:** Tag-driven pipeline, AUR quartet and a budget referee ([a3292bd](https://github.com/CPT-Dawn/Hoarder/commit/a3292bde2c42bcb6519afdce2ebaade20dc5dc21))
- **release:** Run container jobs in bash, give builder a real HOME ([684380c](https://github.com/CPT-Dawn/Hoarder/commit/684380c4ff25bbe4458b918245fedc3d9f2957f4))
- **shm:** Give 4K frames room to breathe in /dev/shm ([39d7a4c](https://github.com/CPT-Dawn/Hoarder/commit/39d7a4ce909b659768fada0ce2221f4d7a3111a1))

### Chores

- **agents:** Teach the robots where the issues live ([6a7d48b](https://github.com/CPT-Dawn/Hoarder/commit/6a7d48bca169ba75f7ae7e73c81cc28d6ab4c7e4))
- **git:** Keep agent worktrees out of the hoard ([6f12b22](https://github.com/CPT-Dawn/Hoarder/commit/6f12b22f1cef660bc748e650729ab6201d2c870a))
- **deny:** Let cosmic-text's retired font parser slide, for now ([635b1bd](https://github.com/CPT-Dawn/Hoarder/commit/635b1bd0f62ddefdb3febd39f20a1dcf25268024))
- **clippy:** Box the clipboard, name the failure ([9729a5d](https://github.com/CPT-Dawn/Hoarder/commit/9729a5dd9fd278060e5915e0adb72084e8974e0e))


