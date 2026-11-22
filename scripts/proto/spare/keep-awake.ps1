# Ver7.0 Phase 0: モダン スタンバイのPCを、画面が消えても眠らせないための試験用スクリプト
# 実行中だけ、OSに「システムを使用中」「実行を続ける必要がある」という要求を出し続ける。
# 閉じる(Ctrl+C か、ウィンドウを閉じる)と、要求は消えて、元の動きに戻る。設定は何も変えない。
# 例: .\keep-awake.ps1
#     .\keep-awake.ps1 -Hours 24
param(
    [double]$Hours = 0   # 0 のときは、閉じるまで続ける
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class KeepAwake
{
    [StructLayout(LayoutKind.Sequential)]
    struct REASON_CONTEXT
    {
        public uint Version;
        public uint Flags;
        public IntPtr SimpleReasonString;
        public IntPtr Pad1;
        public IntPtr Pad2;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr PowerCreateRequest(ref REASON_CONTEXT context);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool PowerSetRequest(IntPtr request, int requestType);

    [DllImport("kernel32.dll")]
    static extern uint SetThreadExecutionState(uint flags);

    const uint ES_CONTINUOUS = 0x80000000;
    const uint ES_SYSTEM_REQUIRED = 0x00000001;
    const int PowerRequestSystemRequired = 1;
    const int PowerRequestExecutionRequired = 3;

    static IntPtr handle = IntPtr.Zero;

    // 要求を出す。戻り値は、登録できた要求の説明
    public static string Start(string reason)
    {
        var ctx = new REASON_CONTEXT();
        ctx.Version = 0;
        ctx.Flags = 0x1; // POWER_REQUEST_CONTEXT_SIMPLE_STRING
        ctx.SimpleReasonString = Marshal.StringToHGlobalUni(reason);
        handle = PowerCreateRequest(ref ctx);
        string result = "";
        if (handle == IntPtr.Zero || handle == new IntPtr(-1))
        {
            result += "PowerCreateRequest失敗(エラー " + Marshal.GetLastWin32Error() + ") ";
        }
        else
        {
            result += PowerSetRequest(handle, PowerRequestSystemRequired) ? "システム要求=成功 " : "システム要求=失敗 ";
            result += PowerSetRequest(handle, PowerRequestExecutionRequired) ? "実行要求=成功 " : "実行要求=失敗 ";
        }
        // 古い書き方の要求も重ねておく(この呼び出しは、呼んだスレッドが生きている間だけ効く)
        uint prev = SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        result += prev == 0 ? "ES=失敗" : "ES=成功";
        return result;
    }
}
'@

$result = [KeepAwake]::Start('Shiori Ver7 prototype server keep-awake test')
Write-Host "要求を出した: $result"
Write-Host "この窓を閉じるか Ctrl+C で、要求は消える。確認(管理者のPowerShell): powercfg /requests"

$end = if ($Hours -gt 0) { (Get-Date).AddHours($Hours) } else { [datetime]::MaxValue }
while ((Get-Date) -lt $end) { Start-Sleep -Seconds 30 }
Write-Host '終わった'
