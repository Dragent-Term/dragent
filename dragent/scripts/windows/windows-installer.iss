; ─────────────────────────────────────────────────────────────────────────────
; Dragent — Windows installer (Inno Setup 6)
;
; Original script for this project. Uses standard Inno Setup directives to
; produce a signed-able .exe installer with the usual wizard flow:
;   Welcome → Destination → Select Additional Tasks (Create a desktop shortcut)
;   → Ready to Install (shows app info + banner/logo) → Installing → Finish
;   (with a "Launch Dragent" checkbox).
;
; Build steps:
;   1. cargo build --release --target x86_64-pc-windows-msvc
;   2. Compile with the Inno Setup 6 command-line compiler:
;        ISCC.exe scripts\windows\windows-installer.iss
;      → scripts\windows\dist\Dragent-<version>-windows-x64-setup.exe
;
; See README.md in this folder for prerequisites and how to regenerate the
; branding images (installer-images\*).
; ─────────────────────────────────────────────────────────────────────────────

#ifndef AppName
  #define AppName "Dragent"
#endif
#define AppPublisher "Dragent"
#define AppUrl       "https://dragent.dev"
#define AppExeName   "dragent.exe"

; The AppId uniquely identifies this application to Windows. KEEP IT CONSTANT
; across releases — installers with the same AppId are treated as upgrades.
; Generate your own once (Inno Setup IDE: Tools ▸ Generate GUID) and paste here.
; Dev builds use {E0000677-10F0-44DA-8BAD-6CE519085F47} (passed as /DAppId).
#ifndef AppId
#define AppId "{{B7E4C9A2-3D51-4F86-9A0C-1E2D3F4A5B60}"
#endif
; Custom URL protocol this install claims (Release: "dragent",
; Dev: "dragent-dev"). The app also registers it at startup; the installer
; entry covers launch-before-first-run and repair flows.
#ifndef UrlScheme
  #define UrlScheme "dragent"
#endif
; Taskbar grouping id. Dev uses "dev.dragent.app.dev" so its windows never
; group with the installed app's.
#ifndef AppUserModelId
  #define AppUserModelId "dev.dragent.app"
#endif

; These may be overridden from the command line (package-windows.ps1 does this):
;   ISCC.exe /DAppVersion=1.2.3 /DReleaseDir=<staged files> /DOutDir=<output> ...
#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
; Folder holding the files to install (dragent.exe, dragent-host.exe, DLLs).
; Defaults to the layout produced by scripts\package-windows.ps1.
#ifndef ReleaseDir
  #define ReleaseDir "..\..\dist\windows-dragent"
#endif
; Where the finished installer .exe is written.
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif
; Target architecture: "x64" (default) or "arm64". Drives the allowed-arch
; gates and the output file name. package-windows.ps1 passes /DArch=… .
#ifndef Arch
  #define Arch "x64"
#endif
#if Arch == "arm64"
  #define ArchAllowed "arm64"
#else
  #define ArchAllowed "x64compatible"
#endif

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
AppUpdatesURL={#AppUrl}

; Install per-user by default (no admin prompt); the user can choose all-users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}

; 64-bit only; arm64 builds install only on Windows-on-ARM.
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}

; Output.
OutputDir={#OutDir}
OutputBaseFilename={#AppName}-{#AppVersion}-windows-{#Arch}-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern

; Branding shown on the Welcome and Ready-to-Install pages (see README to
; regenerate). The large banner is the left/side image; the small one is the
; header logo on later pages.
WizardImageFile=installer-images\dragent-banner.bmp
WizardSmallImageFile=installer-images\dragent-logo.bmp
; Icon for the installer .exe itself (same multi-res .ico as the app).
SetupIconFile=installer-images\dragent.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; This is the "Select Additional Tasks" wizard page. `CreateDesktopIcon` and
; `AdditionalIcons` are built-in Inno messages, so the label is localized for us.
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#ReleaseDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; The agent session helper, found beside dragent.exe. Without it agents start
; without the hooks that report working / review / failed, so neither status
; nor notifications work. Required: a build missing it fails here.
Source: "{#ReleaseDir}\dragent-host.exe"; DestDir: "{app}"; Flags: ignoreversion
; Any graphics-runtime DLLs shipped beside the exe (DirectX/Vulkan), if present.
Source: "{#ReleaseDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
; Built-in agent skills (skill discovery reads {app}\skills), if staged.
Source: "{#ReleaseDir}\skills\*"; DestDir: "{app}\skills"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Registry]
; Custom URL protocol handler (Release: dragent://, Dev: dragent-dev://).
; The app also registers its edition's scheme at startup; the installer
; entry covers link delivery before the first run and repair flows.
Root: HKCU; Subkey: "Software\Classes\{#UrlScheme}"; ValueType: string; ValueName: ""; ValueData: "{#AppName} URL"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\{#UrlScheme}"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\{#UrlScheme}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#UrlScheme}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""; Flags: uninsdeletekey

[Icons]
; Shortcuts use the icon embedded in dragent.exe (attached via winres at build
; time), so no separate .ico is needed here. AppUserModelID keeps all of the
; app's windows grouped under one taskbar button (and is the anchor a future
; taskbar "jump list" would hang off — the Windows analogue of the macOS dock menu).
Name: "{group}\{#AppName}";       Filename: "{app}\{#AppExeName}"; AppUserModelID: "{#AppUserModelId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "{#AppUserModelId}"; Tasks: desktopicon

[Run]
; "Launch Dragent" checkbox on the final wizard page.
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; Auto-update relaunch: when the app hands off to this installer with
; /update=1 (silent in-place upgrade), restart Dragent once the upgrade is done.
Filename: "{app}\{#AppExeName}"; Flags: nowait; Check: IsUpdateMode

[Code]
function IsUpdateMode: Boolean;
begin
  Result := ExpandConstant('{param:update|0}') = '1';
end;
