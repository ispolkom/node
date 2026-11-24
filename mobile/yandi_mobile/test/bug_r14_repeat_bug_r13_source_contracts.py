"""Source-only integration contracts; not widget/device execution.
Checks connect app call sites to semantics verified in installed Android plugin sources.
These assertions flag the two unimplemented paths; they do not prove runtime navigation/wipe.
"""
from pathlib import Path
APP=Path(__file__).resolve().parents[1]
LIB=APP/'lib'
failed=0

def check(name, condition, message):
 global failed
 if condition: print('PASS '+name)
 else: failed+=1;print('FAIL '+name+': '+message)

screens=[LIB/'screens/files_tab.dart',LIB/'screens/settings_screen.dart']
for file in screens:
 text=file.read_text()
 assert 'clearTemporaryFiles()' in text, 'Review needed: picker flow changed'
check('bug_r13_picker_import_and_avatar_must_wipe_before_plugin_unlink',
 all(any(call in f.read_text() for call in ['dropPickerCopy(', 'wipeFile(']) for f in screens),
 'Both picker screens call clearTemporaryFiles without a wipe path; installed FileUtils.clearCache recursively calls File.delete')
all_sources='\n'.join(f.read_text() for f in LIB.rglob('*.dart'))
check('bug_r13_cold_start_notification_must_capture_launch_payload',
 'getNotificationAppLaunchDetails(' in all_sources,
 'No launch-intent payload retrieval; installed Android initialize only registers settings, callback comes from onNewIntent')
print(f'RESULT {2-failed} PASS / {failed} FAIL (source contracts only)')
raise SystemExit(int(failed>0))
