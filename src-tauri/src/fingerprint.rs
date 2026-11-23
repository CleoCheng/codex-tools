use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use futures_util::stream;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::account_service;
use crate::auth;
use crate::auto_switch;
use crate::models::AccountFingerprintProgressEvent;
use crate::models::AccountFingerprintSummary;
use crate::models::AccountSourceKind;
use crate::models::ModelFingerprintProbe;
use crate::models::ProbeAccountFingerprintsResult;
use crate::models::StoredAccount;
use crate::models::UsageSnapshot;
use crate::models::UsageWindow;
use crate::proxy_service;
use crate::proxy_service::ProxySentinelDispatch;
use crate::proxy_service::ProxySentinelObservation;
use crate::state::AppState;
use crate::store::load_store;
use crate::store::save_store;
use crate::tray;
use crate::utils::now_unix_seconds;
use crate::utils::truncate_for_error;

const DETECTOR_BYTES: &[u8] = include_bytes!("fingerprint_data/shared_detector.bin");
const BANK_META_JSON: &str = include_str!("fingerprint_data/bank_meta.json");

pub(crate) const PROBE_MODEL_SOL: &str = "gpt-6.2-sol";
pub(crate) const PROBE_MODEL_ASTRA: &str = "gpt-6.2-astra";
pub(crate) const PROBE_REASONING_EFFORT: &str = "xhigh";
pub(crate) const DEFAULT_PROBE_MODEL: &str = "6.2sol/6.2astra";
pub(crate) const ACCOUNT_FINGERPRINT_PROGRESS_EVENT: &str = "account-fingerprint-progress";

const PROBE_CLIENT_VERSION: &str = "0.180.0";
const PROBE_USER_AGENT: &str = "codex_cli_rs/0.180.0";
const PROBE_SAMPLE_COUNT: usize = 3;
const FAST_PATH_MIN_SAMPLES: usize = 2;
const FAST_PATH_MIN_SCORE: f64 = 3.0;
const FAST_PATH_MIN_MARGIN: f64 = 1.5;
const PROBE_ACCOUNT_CONCURRENCY: usize = 4;
const PROBE_ATTEMPT_TIMEOUT_SECS: u64 = 150;
const PROBE_MAX_ATTEMPTS: usize = 3;
const MAX_PROBE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

const SENTINEL_CONSECUTIVE_THRESHOLD: usize = 3;
const SENTINEL_COOLDOWN_SECS: i64 = 15 * 60;
const SENTINEL_MISMATCH_QUARANTINE_SECS: i64 = 5 * 60;
const SENTINEL_PROBING_QUARANTINE_SECS: i64 = 3 * 60;
const SENTINEL_MIN_INPUT_TOKENS: i64 = 800;
const SENTINEL_MIN_OUTPUT_TOKENS: i64 = 40;
const SENTINEL_LOW_REASONING_THRESHOLD: i64 = 16;
const SENTINEL_HEALTHY_REASONING_THRESHOLD: i64 = 64;
const SENTINEL_AUDIT_HEALTHY_CALL_INTERVAL: usize = 6;

const RECOVERY_BACKOFF_SECS: [i64; 5] = [60 * 60, 2 * 60 * 60, 4 * 60 * 60, 8 * 60 * 60, 12 * 60 * 60];
const RECOVERY_MIN_INTERVAL_SECS: i64 = 15 * 60;
const RECOVERY_TRANSIENT_RETRY_SECS: i64 = 30 * 60;
const RECOVERY_WINDOW_RESET_LEAD_SECS: i64 = 60;
const RECOVERY_MAX_FIVE_HOUR_USED_PERCENT: f64 = 90.0;
const RECOVERY_PRE_RESET_DEFER_FIVE_HOUR_PERCENT: f64 = 80.0;
const RECOVERY_MAX_ONE_WEEK_USED_PERCENT: f64 = 95.0;
const RECOVERY_MAX_PROBES_PER_CYCLE: usize = 1;

const VALUE_MIN: usize = 1;
const VALUE_MAX: usize = 355;
const DIMENSION: usize = VALUE_MAX - VALUE_MIN + 1;
const ALPHA: f64 = 0.5;
const EPS: f64 = 1e-12;
const POSITIONAL_WEIGHTS: [f64; 4] = [0.6, 0.15, 0.125, 0.125];
const LAGS: [usize; 8] = [1, 2, 3, 5, 8, 13, 21, 34];

const SEQ_LIMIT_AUTOCORRELATION: f64 = -0.23;
const SEQ_LIMIT_SMALL_STEP_SHARE: f64 = 0.05;
const SEQ_LIMIT_NEARBY_PAIR_SHARE: f64 = 0.07;

#[derive(Deserialize)]
struct BankMetaFile {
    built_at: String,
    models: Vec<BankModelMeta>,
}

#[derive(Deserialize)]
struct BankModelMeta {
    id: String,
    display_name: String,
    family: String,
    family_name: String,
    response_count: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EncodedFloats {
    Ref {
        #[serde(rename = "$f32")]
        offset: usize,
        n: usize,
    },
    Inline(Vec<f64>),
}

impl EncodedFloats {
    fn resolve(self, floats: &[f32]) -> Result<Vec<f64>, String> {
        match self {
            EncodedFloats::Ref { offset, n } => {
                let end = offset
                    .checked_add(n)
                    .ok_or_else(|| "指纹模型偏移溢出".to_string())?;
                let slice = floats
                    .get(offset..end)
                    .ok_or_else(|| format!("指纹模型切片越界: {offset}..{end}"))?;
                Ok(slice.iter().map(|&v| v as f64).collect())
            }
            EncodedFloats::Inline(v) => Ok(v),
        }
    }
}

#[derive(Deserialize)]
struct RawMeanScale {
    mean: EncodedFloats,
    scale: EncodedFloats,
}

#[derive(Deserialize)]
struct RawHellingerBank {
    feature_mean: EncodedFloats,
    feature_scale: EncodedFloats,
    nuisance_basis: Vec<EncodedFloats>,
    centroids: Vec<EncodedFloats>,
}

#[derive(Deserialize)]
struct RawOrderedBlocksBank {
    feature_mean: EncodedFloats,
    feature_scale: EncodedFloats,
    nuisance_basis: Vec<EncodedFloats>,
    centroids: Vec<EncodedFloats>,
    environment_centroids: Vec<Vec<EncodedFloats>>,
}

#[derive(Deserialize)]
struct RawBank {
    hellinger: RawHellingerBank,
    ordered_blocks: RawOrderedBlocksBank,
}

#[derive(Deserialize)]
struct RawPositional {
    params: Vec<RawMeanScale>,
    weights: Vec<EncodedFloats>,
    bias: EncodedFloats,
}

#[derive(Deserialize)]
struct RawBlend {
    frequency: f64,
    positional: f64,
}

#[derive(Deserialize)]
struct RawRanker {
    head_params: Vec<RawMeanScale>,
    full_params: Vec<RawMeanScale>,
    lda_weights: Vec<EncodedFloats>,
    lda_bias: EncodedFloats,
    references: Vec<Vec<EncodedFloats>>,
    bank: RawBank,
    blend: RawBlend,
    positional: RawPositional,
}

#[derive(Deserialize)]
struct RawCalibrationBinding {
    base_sha256: String,
    reference_sha256: String,
    model_ids: Vec<String>,
}

#[derive(Deserialize)]
struct RawCalibration {
    schema: String,
    method: String,
    tau: f64,
    binding: Option<RawCalibrationBinding>,
}

#[derive(Deserialize)]
struct RawArtifact {
    schema: String,
    base_sha256: String,
    source_reference_sha256: String,
    bank_built_at: String,
    model_ids: Vec<String>,
    response_counts: EncodedFloats,
    ranker: RawRanker,
    calibration: Option<RawCalibration>,
}

struct MeanScale {
    mean: Vec<f64>,
    scale: Vec<f64>,
}

struct HellingerBank {
    feature_mean: Vec<f64>,
    feature_scale: Vec<f64>,
    nuisance_basis: Vec<Vec<f64>>,
    centroids: Vec<Vec<f64>>,
}

struct OrderedBlocksBank {
    feature_mean: Vec<f64>,
    feature_scale: Vec<f64>,
    nuisance_basis: Vec<Vec<f64>>,
    centroids: Vec<Vec<f64>>,
    environment_centroids: Vec<Vec<Vec<f64>>>,
}

struct DetectorBank {
    hellinger: HellingerBank,
    ordered_blocks: OrderedBlocksBank,
}

struct PositionalHead {
    params: Vec<MeanScale>,
    weights: Vec<Vec<f64>>,
    bias: Vec<f64>,
}

struct Ranker {
    head_params: Vec<MeanScale>,
    full_params: Vec<MeanScale>,
    lda_weights: Vec<Vec<f64>>,
    lda_bias: Vec<f64>,
    references: Vec<Vec<Vec<f64>>>,
    reference_norms: Vec<Vec<f64>>,
    bank: DetectorBank,
    blend_frequency: f64,
    blend_positional: f64,
    positional: PositionalHead,
}

struct Calibration {
    tau: f64,
}

struct LoadedDetector {
    models: Vec<BankModelMeta>,
    ranker: Ranker,
    calibration: Option<Calibration>,
}

#[derive(Debug, Clone)]
pub(crate) struct OutputSample {
    pub(crate) text: String,
    pub(crate) expected_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelScore {
    pub(crate) model: String,
    pub(crate) display_name: String,
    pub(crate) score: f64,
    pub(crate) probability: Option<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct DetectionAnalysis {
    pub(crate) decision: &'static str,
    pub(crate) used_outputs: usize,
    pub(crate) results: Vec<ModelScore>,
}

#[derive(Debug, Clone)]
struct ChallengeSpec {
    expected_count: usize,
    prompt: String,
}

struct ChallengeLanguage {
    space: &'static str,
    openings: &'static [&'static str],
    actions: &'static [&'static str],
    request_prefix: &'static str,
    request_middle: &'static str,
    request_suffix: &'static str,
    rules: &'static str,
    endings: &'static [&'static str],
    separators: &'static [&'static str],
    start: &'static str,
}

const LANGUAGES: [ChallengeLanguage; 5] = [
    ChallengeLanguage {
        space: "",
        openings: &[
            "这是一次独立的数值选择记录。",
            "请完成下面的无语义整数选择任务。",
            "执行一次第一反应取值记录。",
            "生成一组不承载语义的整数选择。",
            "进行一轮快速逐项取值。",
        ],
        actions: &[
            "为各个位置分别凭第一反应选择",
            "逐项选择",
            "每次只决定当前一项，共给出",
            "分别凭第一反应给出",
            "逐个直接选择",
        ],
        request_prefix: "",
        request_middle: " ",
        request_suffix: " 个 1 到 355（含端点）的整数。",
        rules: "每个位置都要单独选择；不要从 1 开始计数，不要连续递增或递减，也不要采用等差、循环、重复区块或其他规则化模式。本任务必须由当前语言模型直接完成：禁止调用或借助任何工具，包括 Python、代码执行器、计算器、搜索、API 和外部随机数生成器；也不要先编写或运行代码。",
        endings: &[
            "允许某个数字再次出现；每项写出后不要回头排序、去重或替换。",
            "偶然重复是有效的；不要重新排列或修正已经写出的项目。",
            "相同值可以再次出现；输出过程中不要整理或改写前面的项目。",
            "重复值无需删除；不要筛选、重排或补成某种规律。",
            "不必赋予数字任何含义；已经给出的值保持不变。",
        ],
        separators: &[
            "数字之间用逗号或空格分隔均可。",
            "使用一种一致的常见分隔符即可。",
            "可以用逗号、空格或换行分隔。",
            "只要每个整数边界清楚，格式可自行选择。",
        ],
        start: "直接从第一个取值开始输出，不要在序列前重复数量、范围或任务说明。",
    },
    ChallengeLanguage {
        space: " ",
        openings: &[
            "This is an independent record of numerical choices.",
            "Complete the following integer-choice task, which carries no semantic meaning.",
            "Record a round of first-instinct choices.",
            "Generate a set of integer choices that carry no meaning.",
            "Make a quick round of item-by-item choices.",
        ],
        actions: &[
            "For each position, choose separately by first instinct",
            "Choose, item by item,",
            "Decide only the current item each time, giving a total of",
            "Give, each by first instinct,",
            "Choose directly, one by one,",
        ],
        request_prefix: "",
        request_middle: " ",
        request_suffix: " integers from 1 to 355, inclusive.",
        rules: "Choose separately for every position; do not count up from 1, do not increase or decrease consecutively, and do not use arithmetic progressions, cycles, repeated blocks, or other systematic patterns. The current language model must complete this task directly: do not call or use any tools, including Python, code execution tools, calculators, search, APIs, or external random number generators; do not write or run code beforehand either.",
        endings: &[
            "A number may appear again; after writing each item, do not go back to sort, deduplicate, or replace it.",
            "Accidental repetitions are valid; do not rearrange or correct items already written.",
            "The same value may appear again; do not organize or rewrite earlier items while producing the output.",
            "Repeated values do not need to be removed; do not filter, reorder, or fill the sequence into a pattern.",
            "Do not assign meaning to the numbers; keep values already given unchanged.",
        ],
        separators: &[
            "Separate the numbers with commas or spaces.",
            "Use one consistent common separator.",
            "You may separate the numbers with commas, spaces, or line breaks.",
            "Choose any format that clearly separates each integer.",
        ],
        start: "Start the output directly with the first chosen value; do not repeat the count, range, or task instructions before the sequence.",
    },
    ChallengeLanguage {
        space: "",
        openings: &[
            "これは独立した数値選択の記録です。",
            "意味を持たない次の整数選択課題を完了してください。",
            "直感で選んだ値を記録してください。",
            "意味を持たない整数の選択を一組生成してください。",
            "一項目ずつ素早く値を選んでください。",
        ],
        actions: &[
            "各位置について個別に直感で、",
            "一項目ずつ、",
            "毎回その時点の一項目だけを決めて、",
            "それぞれ直感で、",
            "一つずつ直接、",
        ],
        request_prefix: "",
        request_middle: "1 から 355 まで（両端を含む）の整数を ",
        request_suffix: " 個選んでください。",
        rules: "各位置の値は個別に選んでください。1 から順に数えたり、連続して増加または減少させたりせず、等差数列、周期、同じブロックの繰り返し、その他の規則的なパターンも使わないでください。この課題は現在の言語モデル自身が直接行ってください。Python、コード実行ツール、計算機、検索、API、外部の乱数生成器など、いかなるツールも呼び出したり利用したりしないでください。事前にコードを書いたり実行したりすることも禁止します。",
        endings: &[
            "同じ数字が再び現れてもかまいません。各項目を書いた後に戻って、並べ替え、重複の削除、置き換えをしないでください。",
            "偶然の重複は有効です。すでに書いた項目を並べ替えたり修正したりしないでください。",
            "同じ値が再び出てもかまいません。出力の途中で前の項目を整理したり書き換えたりしないでください。",
            "重複した値を削除する必要はありません。選別、並べ替え、規則的なパターンへの補完をしないでください。",
            "数字に意味を持たせる必要はありません。すでに出した値は変えないでください。",
        ],
        separators: &[
            "数字はコンマまたは空白で区切ってください。",
            "一般的な区切り文字を一種類、統一して使ってください。",
            "コンマ、空白、改行のいずれかで数字を区切ってください。",
            "各整数の区切りが明確であれば、形式は自由です。",
        ],
        start: "選んだ最初の値から直接出力を始め、数列の前に個数、範囲、課題の説明を繰り返さないでください。",
    },
    ChallengeLanguage {
        space: " ",
        openings: &[
            "이것은 독립적인 수치 선택 기록입니다.",
            "다음의 의미 없는 정수 선택 과제를 완료하세요.",
            "첫 직감에 따른 값 선택을 한 번 기록하세요.",
            "의미를 담지 않은 정수 선택을 한 세트 생성하세요.",
            "항목별로 빠르게 값을 고르는 라운드를 진행하세요.",
        ],
        actions: &[
            "각 위치마다 첫 직감으로 따로",
            "항목별로",
            "매번 현재 항목 하나만 정하면서",
            "각각 첫 직감으로",
            "하나씩 바로",
        ],
        request_prefix: "",
        request_middle: " 1부터 355까지(양 끝 포함)의 정수 ",
        request_suffix: "개를 고르세요.",
        rules: "각 위치의 값은 따로 고르세요. 1부터 차례로 세지 말고, 연속으로 증가하거나 감소하지 말며, 등차수열, 순환, 반복되는 블록 또는 그 밖의 규칙적인 패턴도 사용하지 마세요. 이 과제는 현재 언어 모델이 직접 수행해야 합니다. Python, 코드 실행기, 계산기, 검색, API, 외부 난수 생성기를 포함한 어떤 도구도 호출하거나 이용하지 마세요. 미리 코드를 작성하거나 실행하지도 마세요.",
        endings: &[
            "같은 숫자가 다시 나와도 됩니다. 각 항목을 쓴 뒤 되돌아가서 정렬하거나 중복을 제거하거나 바꾸지 마세요.",
            "우연한 중복도 유효합니다. 이미 쓴 항목을 다시 배열하거나 고치지 마세요.",
            "같은 값이 다시 나와도 됩니다. 출력하는 동안 앞의 항목을 정리하거나 고쳐 쓰지 마세요.",
            "중복된 값을 지울 필요는 없습니다. 걸러 내거나 다시 배열하거나 어떤 규칙에 맞춰 채우지 마세요.",
            "숫자에 어떤 의미도 부여할 필요가 없습니다. 이미 제시한 값은 그대로 두세요.",
        ],
        separators: &[
            "숫자는 쉼표나 공백으로 구분하면 됩니다.",
            "일반적인 구분 기호 하나를 일관되게 사용하세요.",
            "쉼표, 공백 또는 줄바꿈으로 구분할 수 있습니다.",
            "각 정수의 경계만 분명하면 형식은 자유롭게 정해도 됩니다.",
        ],
        start: "고른 첫 번째 값부터 바로 출력하고, 수열 앞에 개수, 범위 또는 과제 설명을 반복하지 마세요.",
    },
    ChallengeLanguage {
        space: " ",
        openings: &[
            "Ceci est un relevé indépendant de choix numériques.",
            "Veuillez accomplir la tâche suivante de choix d’entiers sans signification sémantique.",
            "Consignez une série de choix faits au premier réflexe.",
            "Générez une série de choix d’entiers dépourvus de sens.",
            "Effectuez une série rapide de choix, élément par élément.",
        ],
        actions: &[
            "Pour chaque position, choisissez séparément au premier réflexe",
            "Choisissez, élément par élément,",
            "À chaque étape, décidez uniquement de la valeur actuelle, pour donner au total",
            "Donnez séparément au premier réflexe",
            "Choisissez directement, un par un,",
        ],
        request_prefix: "",
        request_middle: " ",
        request_suffix: " entiers compris entre 1 et 355, bornes incluses.",
        rules: "Choisissez séparément pour chaque position ; ne comptez pas à partir de 1, ne produisez pas de valeurs consécutives croissantes ou décroissantes, et ne suivez pas de progression arithmétique, de cycle, de blocs répétés ou d’autre motif systématique. Le modèle de langage actuel doit accomplir cette tâche directement : n’appelez et n’utilisez aucun outil, notamment Python, un exécuteur de code, une calculatrice, une recherche, une API ou un générateur externe de nombres aléatoires ; n’écrivez et n’exécutez pas de code au préalable.",
        endings: &[
            "Un nombre peut réapparaître ; après avoir écrit chaque élément, ne revenez pas en arrière pour le trier, supprimer les doublons ou le remplacer.",
            "Les répétitions accidentelles sont valides ; ne réorganisez et ne corrigez pas les éléments déjà écrits.",
            "Une même valeur peut réapparaître ; pendant la sortie, ne réorganisez et ne réécrivez pas les éléments précédents.",
            "Les valeurs répétées n’ont pas besoin d’être supprimées ; ne filtrez pas, ne réordonnez pas et ne complétez pas la séquence pour former un motif.",
            "N’attribuez aucune signification aux nombres ; gardez inchangées les valeurs déjà données.",
        ],
        separators: &[
            "Séparez les nombres par des virgules ou des espaces.",
            "Utilisez un séparateur courant et cohérent.",
            "Vous pouvez séparer les nombres par des virgules, des espaces ou des retours à la ligne.",
            "Choisissez librement le format, pourvu que la limite de chaque entier soit claire.",
        ],
        start: "Commencez directement par la première valeur choisie, sans répéter le nombre d’éléments, l’intervalle ou les consignes avant la séquence.",
    },
];

static LOADED_DETECTOR: OnceLock<Result<LoadedDetector, String>> = OnceLock::new();

fn resolve_mean_scales(raw: Vec<RawMeanScale>, floats: &[f32]) -> Result<Vec<MeanScale>, String> {
    raw.into_iter()
        .map(|item| {
            Ok(MeanScale {
                mean: item.mean.resolve(floats)?,
                scale: item.scale.resolve(floats)?,
            })
        })
        .collect()
}

fn resolve_matrix(raw: Vec<EncodedFloats>, floats: &[f32]) -> Result<Vec<Vec<f64>>, String> {
    raw.into_iter().map(|v| v.resolve(floats)).collect()
}

fn decode_loaded_detector() -> Result<LoadedDetector, String> {
    if DETECTOR_BYTES.len() < 4 {
        return Err("内置指纹检测模型文件损坏".to_string());
    }
    let header_len = u32::from_le_bytes([
        DETECTOR_BYTES[0],
        DETECTOR_BYTES[1],
        DETECTOR_BYTES[2],
        DETECTOR_BYTES[3],
    ]) as usize;
    let header_end = 4usize
        .checked_add(header_len)
        .ok_or_else(|| "指纹模型头长度无效".to_string())?;
    let data_start = header_end.div_ceil(4) * 4;
    if data_start > DETECTOR_BYTES.len() || (DETECTOR_BYTES.len() - data_start) % 4 != 0 {
        return Err("指纹模型数据体长度无效".to_string());
    }

    let float_bytes = &DETECTOR_BYTES[data_start..];
    let count = float_bytes.len() / 4;
    let mut floats = Vec::with_capacity(count);
    for i in 0..count {
        let bits = [
            float_bytes[i],
            float_bytes[count + i],
            float_bytes[2 * count + i],
            float_bytes[3 * count + i],
        ];
        floats.push(f32::from_le_bytes(bits));
    }

    let header_str = std::str::from_utf8(&DETECTOR_BYTES[4..header_end])
        .map_err(|e| format!("指纹模型头不是合法 UTF-8: {e}"))?;
    let raw: RawArtifact =
        serde_json::from_str(header_str).map_err(|e| format!("解析指纹模型头失败: {e}"))?;
    let bank_meta: BankMetaFile =
        serde_json::from_str(BANK_META_JSON).map_err(|e| format!("解析模型元数据失败: {e}"))?;
    let response_counts = raw.response_counts.resolve(&floats)?;

    if raw.schema != "shared-detector-v2"
        || bank_meta.built_at != raw.bank_built_at
        || bank_meta.models.len() != raw.model_ids.len()
        || bank_meta.models.len() != response_counts.len()
        || !bank_meta.models.iter().enumerate().all(|(i, m)| {
            m.id == raw.model_ids[i] && (m.response_count as f64 - response_counts[i]).abs() < 0.5
        })
    {
        return Err("内置参考模型元数据与检测器不匹配".to_string());
    }

    let calibration = raw.calibration.and_then(|cal| {
        let binding = cal.binding?;
        if cal.schema == "shared-confidence-v2"
            && cal.method == "ranking-temperature"
            && cal.tau.is_finite()
            && (0.001..=1000.0).contains(&cal.tau)
            && binding.base_sha256 == raw.base_sha256
            && binding.reference_sha256 == raw.source_reference_sha256
            && binding.model_ids == raw.model_ids
        {
            Some(Calibration { tau: cal.tau })
        } else {
            None
        }
    });

    let references = raw
        .ranker
        .references
        .into_iter()
        .map(|model_refs| resolve_matrix(model_refs, &floats))
        .collect::<Result<Vec<_>, _>>()?;
    let reference_norms = references
        .iter()
        .map(|model_refs| model_refs.iter().map(|r| dot(r, r)).collect())
        .collect();

    let environment_centroids = raw
        .ranker
        .bank
        .ordered_blocks
        .environment_centroids
        .into_iter()
        .map(|rows| resolve_matrix(rows, &floats))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(LoadedDetector {
        models: bank_meta.models,
        ranker: Ranker {
            head_params: resolve_mean_scales(raw.ranker.head_params, &floats)?,
            full_params: resolve_mean_scales(raw.ranker.full_params, &floats)?,
            lda_weights: resolve_matrix(raw.ranker.lda_weights, &floats)?,
            lda_bias: raw.ranker.lda_bias.resolve(&floats)?,
            references,
            reference_norms,
            bank: DetectorBank {
                hellinger: HellingerBank {
                    feature_mean: raw.ranker.bank.hellinger.feature_mean.resolve(&floats)?,
                    feature_scale: raw.ranker.bank.hellinger.feature_scale.resolve(&floats)?,
                    nuisance_basis: resolve_matrix(
                        raw.ranker.bank.hellinger.nuisance_basis,
                        &floats,
                    )?,
                    centroids: resolve_matrix(raw.ranker.bank.hellinger.centroids, &floats)?,
                },
                ordered_blocks: OrderedBlocksBank {
                    feature_mean: raw
                        .ranker
                        .bank
                        .ordered_blocks
                        .feature_mean
                        .resolve(&floats)?,
                    feature_scale: raw
                        .ranker
                        .bank
                        .ordered_blocks
                        .feature_scale
                        .resolve(&floats)?,
                    nuisance_basis: resolve_matrix(
                        raw.ranker.bank.ordered_blocks.nuisance_basis,
                        &floats,
                    )?,
                    centroids: resolve_matrix(raw.ranker.bank.ordered_blocks.centroids, &floats)?,
                    environment_centroids,
                },
            },
            blend_frequency: raw.ranker.blend.frequency,
            blend_positional: raw.ranker.blend.positional,
            positional: PositionalHead {
                params: resolve_mean_scales(raw.ranker.positional.params, &floats)?,
                weights: resolve_matrix(raw.ranker.positional.weights, &floats)?,
                bias: raw.ranker.positional.bias.resolve(&floats)?,
            },
        },
        calibration,
    })
}

fn detector() -> Result<&'static LoadedDetector, String> {
    match LOADED_DETECTOR.get_or_init(decode_loaded_detector) {
        Ok(loaded) => Ok(loaded),
        Err(err) => Err(err.clone()),
    }
}

pub(crate) fn parse_numbers(text: &str) -> Vec<u32> {
    let bytes = text.as_bytes();
    let mut runs: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut previous_end = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let separator = &text[previous_end..start];
        if !current.is_empty() && separator.chars().any(char::is_alphabetic) {
            runs.push(std::mem::take(&mut current));
        }
        if let Ok(value) = text[start..i].parse::<u32>() {
            if (VALUE_MIN as u32..=VALUE_MAX as u32).contains(&value) {
                current.push(value);
            }
        }
        previous_end = i;
    }

    if !current.is_empty() {
        runs.push(current);
    }

    runs.into_iter()
        .fold(Vec::new(), |best, run| if run.len() > best.len() { run } else { best })
}

fn count_numbers(numbers: &[u32]) -> [usize; DIMENSION] {
    let mut counts = [0usize; DIMENSION];
    for &number in numbers {
        counts[(number as usize) - VALUE_MIN] += 1;
    }
    counts
}

fn hellinger_feature(counts: &[usize; DIMENSION]) -> Vec<f64> {
    let count_sum: usize = counts.iter().sum();
    let total = count_sum as f64 + ALPHA * DIMENSION as f64;
    counts
        .iter()
        .map(|&value| ((value as f64 + ALPHA) / total).sqrt())
        .collect()
}

fn ordered_block_feature(numbers: &[u32]) -> Vec<f64> {
    let mut pieces = Vec::with_capacity(4 * 16 + 10);
    let base = numbers.len() / 4;
    let remainder = numbers.len() % 4;
    let mut start = 0usize;
    for chunk_idx in 0..4 {
        let size = base + usize::from(chunk_idx < remainder);
        let chunk = &numbers[start..start + size];
        start += size;
        let mut bins = [0.5f64; 16];
        for &value in chunk {
            let idx = ((((value - 1) as f64 / DIMENSION as f64) * 16.0).floor() as usize).min(15);
            bins[idx] += 1.0;
        }
        let total: f64 = bins.iter().sum();
        for b in bins {
            pieces.push((b / total).sqrt());
        }
    }

    let mut last_digits = [0.5f64; 10];
    for &value in numbers {
        last_digits[(value % 10) as usize] += 1.0;
    }
    let last_total: f64 = last_digits.iter().sum();
    for b in last_digits {
        pieces.push((b / last_total).sqrt());
    }
    pieces
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn norm(values: &[f64]) -> Vec<f64> {
    let n = dot(values, values).sqrt().max(EPS);
    values.iter().map(|x| x / n).collect()
}

fn z_score(values: &[f64]) -> Vec<f64> {
    let m = mean(values);
    let var = values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / values.len() as f64;
    let s = var.sqrt().max(EPS);
    values.iter().map(|x| (x - m) / s).collect()
}

fn column_mean(rows: &[Vec<f64>]) -> Vec<f64> {
    if rows.is_empty() {
        return Vec::new();
    }
    let cols = rows[0].len();
    let count = rows.len() as f64;
    let mut out = vec![0.0; cols];
    for row in rows {
        for (i, &val) in row.iter().enumerate() {
            out[i] += val;
        }
    }
    for v in &mut out {
        *v /= count;
    }
    out
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) * 0.5
    }
}

fn blocks(numbers: &[u32]) -> [Vec<f64>; 2] {
    [
        hellinger_feature(&count_numbers(numbers)),
        ordered_block_feature(numbers),
    ]
}

fn transform_blocks(b: &[Vec<f64>; 2], params: &[MeanScale]) -> Vec<f64> {
    let mut out = Vec::new();
    for (j, vec) in b.iter().enumerate() {
        let p = &params[j];
        let scaled: Vec<f64> = vec
            .iter()
            .enumerate()
            .map(|(i, &x)| (x - p.mean[i]) / p.scale[i])
            .collect();
        let weight = if j == 0 { 0.75f64.sqrt() } else { 0.25f64.sqrt() };
        for v in norm(&scaled) {
            out.push(v * weight);
        }
    }
    out
}

fn centered(v: &[f64], mean: &[f64], scale: &[f64]) -> Vec<f64> {
    v.iter()
        .enumerate()
        .map(|(i, &x)| (x - mean[i]) / scale[i])
        .collect()
}

fn subtract_basis(v: &[f64], basis: &[Vec<f64>]) -> Vec<f64> {
    let coefficients: Vec<f64> = basis.iter().map(|row| dot(v, row)).collect();
    v.iter()
        .enumerate()
        .map(|(i, &x)| {
            let proj: f64 = basis
                .iter()
                .enumerate()
                .map(|(j, row)| coefficients[j] * row[i])
                .sum();
            x - proj
        })
        .collect()
}

fn nearest(x: &[f64], references: &[Vec<f64>], norms: &[f64]) -> f64 {
    let xx = dot(x, x);
    let mut dists: Vec<f64> = references
        .iter()
        .enumerate()
        .map(|(i, r)| (xx + norms[i] - 2.0 * dot(x, r)).max(0.0))
        .collect();
    dists.sort_by(|a, b| a.total_cmp(b));
    let k = dists.len().min(7);
    mean(&dists[..k])
}

fn baseline(b: &[Vec<f64>; 2], bank: &DetectorBank) -> Vec<f64> {
    let h = &bank.hellinger;
    let o = &bank.ordered_blocks;
    let h_centered = centered(&b[0], &h.feature_mean, &h.feature_scale);
    let h_unit = norm(&subtract_basis(&h_centered, &h.nuisance_basis));
    let marginal: Vec<f64> = h.centroids.iter().map(|row| dot(&h_unit, row)).collect();

    let o_centered = centered(&b[1], &o.feature_mean, &o.feature_scale);
    let raw = norm(&o_centered);
    let projected = norm(&subtract_basis(&o_centered, &o.nuisance_basis));
    let templates: Vec<f64> = (0..o.centroids.len())
        .map(|i| {
            o.environment_centroids
                .iter()
                .map(|rows| dot(&raw, &rows[i]))
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect();
    let nuisance: Vec<f64> = o.centroids.iter().map(|row| dot(&projected, row)).collect();

    let tz = z_score(&templates);
    let nz = z_score(&nuisance);
    let combined_ordered: Vec<f64> = tz
        .iter()
        .zip(nz.iter())
        .map(|(a, b)| 0.5 * a + 0.5 * b)
        .collect();
    let ordered = z_score(&combined_ordered);
    let mz = z_score(&marginal);
    mz.iter()
        .zip(ordered.iter())
        .map(|(a, b)| 0.5 * a + 0.5 * b)
        .collect()
}

fn position_kernel(numbers: &[u32]) -> Vec<f64> {
    let n = numbers.len() as f64;
    let mut out = vec![0.0; 34 * 3];
    for (position, &value) in numbers.iter().enumerate() {
        let t = (position as f64 + 0.5) * 2.0 / n - 1.0;
        let basis = [
            t,
            0.5 * (3.0 * t * t - 1.0),
            0.5 * (5.0 * t.powi(3) - 3.0 * t),
        ];
        let val_f = value as f64;
        for k in 0..24 {
            let centre = 1.0 + (k as f64) * 354.0 / 23.0;
            let weight = (-0.5 * ((val_f - centre) / 20.0).powi(2)).exp();
            for j in 0..3 {
                out[k * 3 + j] += weight * basis[j] / n;
            }
        }
        let digit = (value % 10) as usize;
        for j in 0..3 {
            out[(24 + digit) * 3 + j] += basis[j] / n;
        }
    }
    out
}

fn centred_transitions(symbols: &[usize], bins: usize) -> Vec<f64> {
    let mut joint = vec![0.5; bins * bins];
    let total = (symbols.len() - 1) as f64 + 0.5 * (bins * bins) as f64;
    for i in 1..symbols.len() {
        joint[symbols[i - 1] * bins + symbols[i]] += 1.0;
    }
    for v in &mut joint {
        *v /= total;
    }
    let mut rows = vec![0.0; bins];
    let mut cols = vec![0.0; bins];
    for a in 0..bins {
        for b in 0..bins {
            let val = joint[a * bins + b];
            rows[a] += val;
            cols[b] += val;
        }
    }
    joint
        .into_iter()
        .enumerate()
        .map(|(i, val)| val - rows[i / bins] * cols[i % bins])
        .collect()
}

fn sequence_features(numbers: &[u32]) -> Vec<f64> {
    let n = numbers.len();
    let xf: Vec<f64> = numbers.iter().map(|&v| v as f64).collect();
    let m = mean(&xf);
    let var = xf.iter().map(|&v| (v - m).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt().max(1.0);
    let s: Vec<f64> = xf.iter().map(|&v| (v - m) / sd).collect();

    let mut out = Vec::with_capacity(220);
    for &lag in &LAGS {
        let count = n - lag;
        let cf = count as f64;
        let mut prod_sum = 0.0;
        let mut abs_sum = 0.0;
        let mut pos_hits = 0usize;
        let mut zero_hits = 0usize;
        let mut le5_hits = 0usize;
        let mut le20_hits = 0usize;
        for i in 0..count {
            let diff = numbers[i + lag] as i32 - numbers[i] as i32;
            prod_sum += s[i + lag] * s[i];
            let abs_diff = diff.unsigned_abs();
            abs_sum += abs_diff as f64;
            if diff > 0 {
                pos_hits += 1;
            }
            if diff == 0 {
                zero_hits += 1;
            }
            if abs_diff <= 5 {
                le5_hits += 1;
            }
            if abs_diff <= 20 {
                le20_hits += 1;
            }
        }
        out.push(prod_sum / cf);
        out.push((abs_sum / cf) / 355.0);
        out.push(pos_hits as f64 / cf);
        out.push(zero_hits as f64 / cf);
        out.push(le5_hits as f64 / cf);
        out.push(le20_hits as f64 / cf);
    }

    let coarse_symbols: Vec<usize> = numbers
        .iter()
        .map(|&v| ((((v - 1) as f64 * 8.0 / 355.0).floor()) as usize).min(7))
        .collect();
    out.extend(centred_transitions(&coarse_symbols, 8));

    let digit_symbols: Vec<usize> = numbers.iter().map(|&v| (v % 10) as usize).collect();
    out.extend(centred_transitions(&digit_symbols, 10));

    let mut same_sign_hits = 0usize;
    let mut same_diff_hits = 0usize;
    for i in 0..(n - 2) {
        let d0 = numbers[i + 1] as i32 - numbers[i] as i32;
        let d1 = numbers[i + 2] as i32 - numbers[i + 1] as i32;
        if d1.signum() == d0.signum() {
            same_sign_hits += 1;
        }
        if d1 == d0 {
            same_diff_hits += 1;
        }
    }
    let mut same_last_digit = 0usize;
    let mut z_gt1 = 0usize;
    let mut z_gt2 = 0usize;
    for i in 0..(n - 1) {
        if numbers[i] % 10 == numbers[i + 1] % 10 {
            same_last_digit += 1;
        }
        let step = (s[i + 1] - s[i]).abs();
        if step > 1.0 {
            z_gt1 += 1;
        }
        if step > 2.0 {
            z_gt2 += 1;
        }
    }
    out.push(same_sign_hits as f64 / (n - 2) as f64);
    out.push(same_diff_hits as f64 / (n - 2) as f64);
    out.push(same_last_digit as f64 / (n - 1) as f64);
    out.push(z_gt1 as f64 / (n - 1) as f64);
    out.push(z_gt2 as f64 / (n - 1) as f64);

    let buckets: Vec<usize> = numbers
        .iter()
        .map(|&v| ((((v - 1) as f64 * 16.0 / 355.0).floor()) as usize).min(15))
        .collect();
    for width in [4usize, 8, 16] {
        let mut sum = 0.0;
        let windows = n - width + 1;
        for i in 0..windows {
            let mut mask = 0u16;
            for &b in &buckets[i..i + width] {
                mask |= 1u16 << b;
            }
            sum += (mask.count_ones() as f64) / (width as f64);
        }
        out.push(sum / (windows as f64));
    }

    out
}

fn positional_decision(numbers: &[u32], head: &PositionalHead) -> Vec<f64> {
    let counts = count_numbers(numbers);
    let parts = [
        hellinger_feature(&counts),
        ordered_block_feature(numbers),
        position_kernel(numbers),
        sequence_features(numbers),
    ];
    let mut x = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for (j, block) in parts.iter().enumerate() {
        let p = &head.params[j];
        let scaled: Vec<f64> = block
            .iter()
            .enumerate()
            .map(|(i, &v)| (v - p.mean[i]) / p.scale[i])
            .collect();
        let len = dot(&scaled, &scaled).sqrt().max(EPS);
        let weight = POSITIONAL_WEIGHTS[j].sqrt();
        for v in scaled {
            x.push((v / len) * weight);
        }
    }
    head.weights
        .iter()
        .enumerate()
        .map(|(k, row)| dot(row, &x) + head.bias[k])
        .collect()
}

fn rank_shared_numbers(numbers: &[Vec<u32>], ranker: &Ranker) -> Result<Vec<f64>, String> {
    let full: Vec<[Vec<f64>; 2]> = numbers.iter().map(|n| blocks(n)).collect();
    let ax: Vec<Vec<f64>> = full
        .iter()
        .map(|b| transform_blocks(b, &ranker.full_params))
        .collect();
    let lda: Vec<Vec<f64>> = numbers
        .iter()
        .map(|n| {
            let head_slice = &n[..n.len().min(128)];
            let x = transform_blocks(&blocks(head_slice), &ranker.head_params);
            let scores: Vec<f64> = ranker
                .lda_weights
                .iter()
                .enumerate()
                .map(|(i, row)| dot(row, &x) + ranker.lda_bias[i])
                .collect();
            z_score(&scores)
        })
        .collect();
    let l = z_score(&column_mean(&lda));
    let near_raw: Vec<f64> = ranker
        .references
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let neg_dists: Vec<f64> = ax
                .iter()
                .map(|x| -nearest(x, r, &ranker.reference_norms[idx]))
                .collect();
            median(&neg_dists)
        })
        .collect();
    let near = z_score(&near_raw);
    let base_rows: Vec<Vec<f64>> = full.iter().map(|b| baseline(b, &ranker.bank)).collect();
    let base = z_score(&column_mean(&base_rows));
    let freq_raw: Vec<f64> = (0..l.len())
        .map(|i| 0.5 * l[i] + 0.25 * near[i] + 0.25 * base[i])
        .collect();
    let frequency = z_score(&freq_raw);

    let pos_rows: Vec<Vec<f64>> = numbers
        .iter()
        .map(|n| z_score(&positional_decision(n, &ranker.positional)))
        .collect();
    let positional = z_score(&column_mean(&pos_rows));

    let ranking: Vec<f64> = frequency
        .iter()
        .zip(positional.iter())
        .map(|(&f, &p)| ranker.blend_frequency * f + ranker.blend_positional * p)
        .collect();
    if ranking.iter().any(|v| !v.is_finite()) {
        return Err("指纹排名计算产生无效数值".to_string());
    }
    Ok(ranking)
}

fn calibrate_ranking(ranking: &[f64], calibration: Option<&Calibration>) -> Option<Vec<f64>> {
    let cal = calibration?;
    if ranking.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let logits: Vec<f64> = ranking.iter().map(|score| cal.tau * score).collect();
    let maximum = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = logits.iter().map(|v| (v - maximum).exp()).collect();
    let sum: f64 = weights.iter().sum();
    Some(weights.into_iter().map(|w| w / sum).collect())
}

fn nearby_pair_share(numbers: &[u32]) -> f64 {
    let mut close = 0usize;
    let mut pairs = 0usize;
    for lag in 1..=3 {
        for i in lag..numbers.len() {
            if numbers[i].abs_diff(numbers[i - lag]) <= 20 {
                close += 1;
            }
            pairs += 1;
        }
    }
    if pairs == 0 {
        0.0
    } else {
        close as f64 / pairs as f64
    }
}

fn sequence_model_like(numbers: &[Vec<u32>]) -> bool {
    let mut autocorrs = Vec::with_capacity(numbers.len());
    let mut small_steps = Vec::with_capacity(numbers.len());
    let mut nearby = Vec::with_capacity(numbers.len());

    for seq in numbers {
        let m = seq.iter().map(|&v| v as f64).sum::<f64>() / seq.len() as f64;
        let variance = seq.iter().map(|&v| (v as f64 - m).powi(2)).sum::<f64>() / seq.len() as f64;
        let autocorr = if variance != 0.0 {
            let cov = (0..(seq.len() - 1))
                .map(|i| (seq[i + 1] as f64 - m) * (seq[i] as f64 - m))
                .sum::<f64>()
                / (seq.len() - 1) as f64;
            cov / variance
        } else {
            1.0
        };
        let small_step_share = (0..(seq.len() - 1))
            .filter(|&i| seq[i + 1].abs_diff(seq[i]) <= 20)
            .count() as f64
            / (seq.len() - 1) as f64;
        autocorrs.push(autocorr);
        small_steps.push(small_step_share);
        nearby.push(nearby_pair_share(seq));
    }

    median(&autocorrs) <= SEQ_LIMIT_AUTOCORRELATION
        || median(&small_steps) <= SEQ_LIMIT_SMALL_STEP_SHARE
        || median(&nearby) <= SEQ_LIMIT_NEARBY_PAIR_SHARE
}

pub(crate) fn analyze_outputs(
    outputs: &[OutputSample],
    allow_partial: bool,
) -> Result<DetectionAnalysis, String> {
    let det = detector()?;
    let mut accepted = Vec::new();
    for output in outputs {
        let nums = parse_numbers(&output.text);
        let minimum = 80usize.max((output.expected_count * 55).div_ceil(100));
        if nums.len() >= minimum {
            accepted.push(nums);
        }
    }
    let used = accepted.len();
    if used == 0 {
        return Ok(DetectionAnalysis {
            decision: "unscorable",
            used_outputs: 0,
            results: Vec::new(),
        });
    }
    if !sequence_model_like(&accepted) {
        return Ok(DetectionAnalysis {
            decision: "not_model",
            used_outputs: used,
            results: Vec::new(),
        });
    }
    let is_partial = used < 3;
    if is_partial && (!allow_partial || outputs.len() > 3) {
        return Ok(DetectionAnalysis {
            decision: "unscorable",
            used_outputs: used,
            results: Vec::new(),
        });
    }

    let ranking = rank_shared_numbers(&accepted, &det.ranker)?;
    let probabilities = if !is_partial && outputs.len() == 3 {
        calibrate_ranking(&ranking, det.calibration.as_ref())
    } else {
        None
    };

    let mut order: Vec<usize> = (0..det.models.len()).collect();
    order.sort_by(|&a, &b| ranking[b].total_cmp(&ranking[a]));
    let results = order
        .into_iter()
        .map(|i| ModelScore {
            model: det.models[i].id.clone(),
            display_name: det.models[i].display_name.clone(),
            score: ranking[i],
            probability: probabilities.as_ref().map(|p| p[i]),
        })
        .collect();

    Ok(DetectionAnalysis {
        decision: if is_partial { "partial" } else { "scored" },
        used_outputs: used,
        results,
    })
}

pub(crate) fn is_model_degraded(
    target_model: &str,
    detected_model_id: &str,
    detected_display_name: &str,
) -> bool {
    let target = target_model.trim().to_ascii_lowercase();
    let id = detected_model_id.trim().to_ascii_lowercase();
    let display = detected_display_name.trim().to_ascii_lowercase();
    let bad_display = display.contains("5.") || display.contains("luna");
    if target == "gpt-6.2-sol" {
        return id != "gpt-6.2-sol" || bad_display;
    }
    if target == "gpt-6.2-astra" {
        return id != "gpt-6.2-astra" || bad_display;
    }
    if target.contains("6.1-sol") || target.contains("gpt-6-astra") {
        let allowed = matches!(id.as_str(), "gpt-6-astra" | "gpt-6.2-astra" | "gpt-6.2-sol");
        return !allowed || bad_display;
    }
    id != target
}

struct SimpleRng(u64);

impl SimpleRng {
    fn from_entropy() -> Self {
        let raw = Uuid::new_v4().as_u128();
        let seed = ((raw as u64) ^ ((raw >> 64) as u64)) | 1;
        Self(seed)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as usize) % bound
    }
}

fn generate_challenges(count: usize) -> Vec<ChallengeSpec> {
    let mut rng = SimpleRng::from_entropy();
    let mut lengths: Vec<usize> = (292..=332).collect();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len_idx = rng.next_usize(lengths.len());
        let expected_count = lengths.swap_remove(len_idx);
        let lang = &LANGUAGES[rng.next_usize(LANGUAGES.len())];
        let opening = lang.openings[rng.next_usize(lang.openings.len())];
        let action = lang.actions[rng.next_usize(lang.actions.len())];
        let request = format!(
            "{}{}{}{}{}",
            lang.request_prefix, action, lang.request_middle, expected_count, lang.request_suffix
        );
        let ending = lang.endings[rng.next_usize(lang.endings.len())];
        let separator = lang.separators[rng.next_usize(lang.separators.len())];
        let parts = [
            opening,
            request.as_str(),
            lang.rules,
            ending,
            separator,
            lang.start,
        ];
        out.push(ChallengeSpec {
            expected_count,
            prompt: parts.join(lang.space),
        });
    }
    out
}

fn extract_completed_text(value: &Value) -> String {
    let mut buf = String::new();
    let Some(items) = value
        .get("response")
        .and_then(|r| r.get("output"))
        .and_then(Value::as_array)
    else {
        return buf;
    };
    for item in items {
        let Some(contents) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for c in contents {
            if let Some(t) = c.get("text").and_then(Value::as_str) {
                buf.push_str(t);
            }
        }
    }
    buf
}

fn is_transient_upstream_message(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("usage_limit")
        || lower.contains("insufficient_quota")
        || lower.contains("invalid_api_key")
        || lower.contains("account_deactivated")
    {
        return false;
    }
    lower.contains("server_is_overloaded")
        || lower.contains("overloaded")
        || lower.contains("rate_limit")
        || lower.contains("too_many_requests")
        || lower.contains("internal_server_error")
        || lower.contains("temporarily_unavailable")
}

fn format_upstream_error(context: &str, raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("usage_limit_reached") || lower.contains("usage_limit") {
        return "该账号额度已耗尽 (usage_limit_reached)，无法发起指纹采样".to_string();
    }
    format!("{context}: {}", truncate_for_error(raw.trim(), 220))
}

fn merge_model_probe(
    prev: Option<&ModelFingerprintProbe>,
    next: Option<&ModelFingerprintProbe>,
) -> Option<ModelFingerprintProbe> {
    match (prev, next) {
        (_, Some(n)) if n.skipped || n.error.is_none() || !n.detected_model.is_empty() => {
            Some(n.clone())
        }
        (Some(p), Some(n)) if !p.detected_model.is_empty() => Some(ModelFingerprintProbe {
            error: n.error.clone(),
            ..p.clone()
        }),
        (Some(p), None) => Some(p.clone()),
        (_, n) => n.cloned(),
    }
}

fn merge_fingerprint_summary(
    prev: Option<&AccountFingerprintSummary>,
    next: &AccountFingerprintSummary,
) -> AccountFingerprintSummary {
    let Some(previous) = prev else {
        return next.clone();
    };
    let sol = merge_model_probe(previous.sol.as_ref(), next.sol.as_ref());
    let astra = merge_model_probe(previous.astra.as_ref(), next.astra.as_ref());

    if next.error.is_some() && next.detected_model.is_empty() && !previous.detected_model.is_empty()
    {
        return AccountFingerprintSummary {
            error: next.error.clone(),
            sol,
            astra,
            ..previous.clone()
        };
    }

    AccountFingerprintSummary {
        sol,
        astra,
        ..next.clone()
    }
}

struct SampleAttemptResult {
    text: String,
    ttft_ms: Option<u64>,
    auth_json: Value,
}

async fn run_single_sample(
    app: &AppHandle,
    state: &AppState,
    account_key: &str,
    mut auth_json: Value,
    client: &reqwest::Client,
    target_model: &str,
    challenge: &ChallengeSpec,
) -> Result<SampleAttemptResult, String> {
    let url = format!(
        "{}/responses",
        crate::proxy_service::resolve_codex_upstream_base_url().trim_end_matches('/')
    );
    let body = serde_json::to_vec(&json!({
        "model": target_model,
        "instructions": "",
        "input": [{ "role": "user", "content": challenge.prompt }],
        "reasoning": { "effort": PROBE_REASONING_EFFORT },
        "store": false,
        "stream": true
    }))
    .map_err(|e| format!("序列化指纹探测请求失败: {e}"))?;

    let mut refreshed_on_401 = false;

    for attempt in 1..=PROBE_MAX_ATTEMPTS {
        let auth = auth::extract_auth(&auth_json)?;
        let started = Instant::now();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", auth.access_token))
            .header("ChatGPT-Account-Id", auth.account_id)
            .header("Accept", "text/event-stream")
            .header("Content-Type", "application/json")
            .header("User-Agent", PROBE_USER_AGENT)
            .header("Originator", "codex_cli_rs")
            .header("Version", PROBE_CLIENT_VERSION)
            .header("session-id", Uuid::new_v4().to_string())
            .body(body.clone())
            .send()
            .await;

        let response = match response {
            Ok(resp) => resp,
            Err(err) => {
                if attempt < PROBE_MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(1500 * attempt as u64)).await;
                    continue;
                }
                return Err(format!("指纹探测请求失败: {err}"));
            }
        };

        let status = response.status();
        if status.as_u16() == 401 && !refreshed_on_401 {
            refreshed_on_401 = true;
            if let Ok(refreshed) = account_service::refresh_account_auth_with_operation_guard(
                app,
                state,
                account_key,
                &auth_json,
            )
            .await
            {
                auth_json = refreshed.auth_json;
                continue;
            }
        }

        if !status.is_success() {
            let err_bytes = response.bytes().await.unwrap_or_default();
            let err_text = String::from_utf8_lossy(&err_bytes);
            if (status.as_u16() == 429 || status.is_server_error())
                && attempt < PROBE_MAX_ATTEMPTS
                && !err_text.to_ascii_lowercase().contains("usage_limit")
            {
                tokio::time::sleep(Duration::from_millis(1500 * attempt as u64)).await;
                continue;
            }
            return Err(format_upstream_error(
                &format!("上游拒绝 HTTP {}", status.as_u16()),
                &err_text,
            ));
        }

        let mut decoder = proxy_service::SseDecoder::default();
        let mut stream = response.bytes_stream();
        let mut total_bytes = 0usize;
        let mut text = String::new();
        let mut ttft_ms = None;
        let mut saw_text_delta = false;
        let mut retryable_overload = None;
        let mut terminal_error = None;
        let mut finished = false;

        while let Some(chunk_res) = stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(err) => {
                    if !saw_text_delta && attempt < PROBE_MAX_ATTEMPTS {
                        retryable_overload = Some(format!("流式读取中断: {err}"));
                    } else if parse_numbers(&text).is_empty() {
                        terminal_error = Some(format!("读取流式响应失败: {err}"));
                    }
                    break;
                }
            };
            total_bytes = total_bytes.saturating_add(chunk.len());
            if total_bytes > MAX_PROBE_RESPONSE_BYTES {
                break;
            }
            for event in decoder.push(&chunk) {
                if event.data.trim() == "[DONE]" {
                    finished = true;
                    break;
                }
                let Ok(val) = serde_json::from_str::<Value>(&event.data) else {
                    continue;
                };
                let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "response.output_text.delta" => {
                        if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                            if !delta.is_empty() {
                                if !saw_text_delta {
                                    ttft_ms = Some(started.elapsed().as_millis() as u64);
                                    saw_text_delta = true;
                                }
                                text.push_str(delta);
                            }
                        }
                    }
                    "response.output_text.done" => {
                        if text.is_empty() {
                            if let Some(full) = val.get("text").and_then(Value::as_str) {
                                text.push_str(full);
                            }
                        }
                    }
                    "response.completed" | "response.done" => {
                        if text.is_empty() {
                            text = extract_completed_text(&val);
                        }
                        finished = true;
                    }
                    "response.failed" | "error" => {
                        let raw_err = val.to_string();
                        let msg = truncate_for_error(&raw_err, 240);
                        if !saw_text_delta
                            && attempt < PROBE_MAX_ATTEMPTS
                            && is_transient_upstream_message(&raw_err)
                        {
                            retryable_overload = Some(msg);
                        } else {
                            terminal_error = Some(format_upstream_error("模型返回失败", &raw_err));
                        }
                        finished = true;
                    }
                    _ => {}
                }

                if parse_numbers(&text).len() >= challenge.expected_count
                    && text.as_bytes().last().is_some_and(|b| !b.is_ascii_digit())
                {
                    finished = true;
                    break;
                }
            }
            if finished || retryable_overload.is_some() || terminal_error.is_some() {
                break;
            }
        }

        if let Some(_reason) = retryable_overload {
            tokio::time::sleep(Duration::from_millis(1500 * attempt as u64)).await;
            continue;
        }
        if let Some(err) = terminal_error {
            return Err(err);
        }

        if !finished {
            for event in decoder.finish() {
                if let Ok(val) = serde_json::from_str::<Value>(&event.data) {
                    if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                        text.push_str(delta);
                    }
                }
            }
        }

        return Ok(SampleAttemptResult {
            text,
            ttft_ms,
            auth_json,
        });
    }

    Err("指纹探测重试次数已耗尽".to_string())
}

fn emit_progress(app: &AppHandle, event: AccountFingerprintProgressEvent) {
    let _ = app.emit(ACCOUNT_FINGERPRINT_PROGRESS_EVENT, event);
}

fn is_fatal_account_probe_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("usage_limit")
        || err.contains("额度已耗尽")
        || err.contains("HTTP 401")
        || err.contains("HTTP 403")
}

fn error_model_probe(
    target_model: &str,
    valid_samples: usize,
    total_samples: usize,
    ttft_ms: Option<u64>,
    err: String,
) -> ModelFingerprintProbe {
    ModelFingerprintProbe {
        target_model: target_model.to_string(),
        effort: PROBE_REASONING_EFFORT.to_string(),
        degraded: false,
        skipped: false,
        detected_model: String::new(),
        detected_model_id: String::new(),
        confidence: None,
        score: None,
        runner_up_model: None,
        runner_up_score: None,
        valid_samples,
        total_samples,
        ttft_ms,
        error: Some(err),
    }
}

fn skipped_model_probe(target_model: &str) -> ModelFingerprintProbe {
    ModelFingerprintProbe {
        target_model: target_model.to_string(),
        effort: PROBE_REASONING_EFFORT.to_string(),
        degraded: false,
        skipped: true,
        detected_model: String::new(),
        detected_model_id: String::new(),
        confidence: None,
        score: None,
        runner_up_model: None,
        runner_up_score: None,
        valid_samples: 0,
        total_samples: 0,
        ttft_ms: None,
        error: None,
    }
}

fn should_early_accept_two_samples(outputs: &[OutputSample]) -> bool {
    if outputs.len() != FAST_PATH_MIN_SAMPLES {
        return false;
    }
    if outputs.iter().any(|sample| sample.text.trim().is_empty()) {
        return false;
    }
    let Ok(analysis) = analyze_outputs(outputs, true) else {
        return false;
    };
    if analysis.used_outputs != FAST_PATH_MIN_SAMPLES || analysis.results.len() < 2 {
        return false;
    }
    let top = &analysis.results[0];
    let runner_up = &analysis.results[1];
    top.score >= FAST_PATH_MIN_SCORE && (top.score - runner_up.score) >= FAST_PATH_MIN_MARGIN
}

async fn probe_single_model_for_account(
    app: &AppHandle,
    state: &AppState,
    account_key: &str,
    mut auth_json: Value,
    client: &reqwest::Client,
    target_model: &str,
    short_label: &str,
    progress_offset: usize,
    total_samples: usize,
    trigger: Option<&str>,
) -> (ModelFingerprintProbe, Value, Option<String>) {
    let challenges = generate_challenges(PROBE_SAMPLE_COUNT);
    let mut outputs = Vec::with_capacity(PROBE_SAMPLE_COUNT);
    let mut ttfts = Vec::with_capacity(PROBE_SAMPLE_COUNT);
    let mut last_error = None;
    let mut fatal_error = None;

    for (idx, challenge) in challenges.iter().enumerate() {
        emit_progress(
            app,
            AccountFingerprintProgressEvent {
                account_key: account_key.to_string(),
                status: "probing".to_string(),
                completed_samples: progress_offset + idx,
                total_samples,
                message: Some(format!(
                    "{short_label} 采样 {}/{}",
                    idx + 1,
                    PROBE_SAMPLE_COUNT
                )),
                summary: None,
                api_proxy_enabled: None,
                trigger: trigger.map(str::to_string),
            },
        );
        match run_single_sample(
            app,
            state,
            account_key,
            auth_json.clone(),
            client,
            target_model,
            challenge,
        )
        .await
        {
            Ok(sample) => {
                auth_json = sample.auth_json;
                if let Some(t) = sample.ttft_ms {
                    ttfts.push(t);
                }
                outputs.push(OutputSample {
                    text: sample.text,
                    expected_count: challenge.expected_count,
                });
                if idx + 1 == FAST_PATH_MIN_SAMPLES
                    && last_error.is_none()
                    && should_early_accept_two_samples(&outputs)
                {
                    break;
                }
            }
            Err(err) => {
                if is_fatal_account_probe_error(&err) {
                    fatal_error = Some(err.clone());
                    last_error = Some(err);
                    break;
                }
                last_error = Some(err);
                outputs.push(OutputSample {
                    text: String::new(),
                    expected_count: challenge.expected_count,
                });
            }
        }
    }

    let avg_ttft = if ttfts.is_empty() {
        None
    } else {
        Some(ttfts.iter().sum::<u64>() / ttfts.len() as u64)
    };

    if let Some(fatal) = fatal_error.as_ref() {
        return (
            error_model_probe(target_model, 0, outputs.len(), avg_ttft, fatal.clone()),
            auth_json,
            fatal_error,
        );
    }

    let attempted_samples = outputs.len().max(1);
    let probe = match analyze_outputs(&outputs, true) {
        Ok(analysis) if !analysis.results.is_empty() => {
            let top = &analysis.results[0];
            let runner_up = analysis.results.get(1);
            let degraded = is_model_degraded(target_model, &top.model, &top.display_name);
            ModelFingerprintProbe {
                target_model: target_model.to_string(),
                effort: PROBE_REASONING_EFFORT.to_string(),
                degraded,
                skipped: false,
                detected_model: top.display_name.clone(),
                detected_model_id: top.model.clone(),
                confidence: top.probability,
                score: Some(top.score),
                runner_up_model: runner_up.map(|r| r.display_name.clone()),
                runner_up_score: runner_up.map(|r| r.score),
                valid_samples: analysis.used_outputs,
                total_samples: attempted_samples,
                ttft_ms: avg_ttft,
                error: None,
            }
        }
        Ok(analysis) => error_model_probe(
            target_model,
            analysis.used_outputs,
            attempted_samples,
            avg_ttft,
            last_error.unwrap_or_else(|| {
                if analysis.decision == "not_model" {
                    "输出数字分布不符合语言模型特征，未完成指纹判定".to_string()
                } else {
                    "有效样本不足，未完成指纹判定".to_string()
                }
            }),
        ),
        Err(err) => error_model_probe(target_model, 0, attempted_samples, avg_ttft, err),
    };

    (probe, auth_json, None)
}

fn compose_account_summary(
    sol: ModelFingerprintProbe,
    astra: ModelFingerprintProbe,
) -> AccountFingerprintSummary {
    let degraded = sol.degraded || (!astra.skipped && astra.degraded);
    let primary = if sol.degraded && !sol.detected_model.is_empty() {
        &sol
    } else if !astra.skipped && astra.degraded && !astra.detected_model.is_empty() {
        &astra
    } else if !sol.detected_model.is_empty() {
        &sol
    } else {
        &astra
    };

    let detected_model = if astra.skipped {
        sol.detected_model.clone()
    } else {
        match (
            sol.detected_model.is_empty(),
            astra.detected_model.is_empty(),
        ) {
            (false, false) => {
                if sol.detected_model_id == astra.detected_model_id {
                    sol.detected_model.clone()
                } else {
                    format!("{} / {}", sol.detected_model, astra.detected_model)
                }
            }
            (false, true) => sol.detected_model.clone(),
            (true, false) => astra.detected_model.clone(),
            (true, true) => String::new(),
        }
    };

    let error = if astra.skipped {
        sol.error.clone()
    } else {
        match (&sol.error, &astra.error) {
            (None, None) => None,
            (Some(e1), Some(e2)) if e1 == e2 => Some(e1.clone()),
            (Some(e1), Some(e2)) => Some(format!("6.2sol: {e1}; 6.2astra: {e2}")),
            (Some(e1), None) => Some(format!("6.2sol: {e1}")),
            (None, Some(e2)) => Some(format!("6.2astra: {e2}")),
        }
    };

    let ttfts: Vec<u64> = [sol.ttft_ms, if astra.skipped { None } else { astra.ttft_ms }]
        .into_iter()
        .flatten()
        .collect();
    let ttft_ms = if ttfts.is_empty() {
        None
    } else {
        Some(ttfts.iter().sum::<u64>() / ttfts.len() as u64)
    };
    let valid_samples = sol.valid_samples + astra.valid_samples;
    let total_samples = sol.total_samples + astra.total_samples;

    AccountFingerprintSummary {
        degraded,
        target_model: DEFAULT_PROBE_MODEL.to_string(),
        detected_model,
        detected_model_id: primary.detected_model_id.clone(),
        confidence: primary.confidence,
        score: primary.score,
        runner_up_model: primary.runner_up_model.clone(),
        runner_up_score: primary.runner_up_score,
        valid_samples,
        total_samples,
        ttft_ms,
        checked_at: now_unix_seconds(),
        error,
        sol: Some(sol),
        astra: Some(astra),
        auto_disabled_by_fingerprint: false,
        auto_disabled_at: None,
        recovery_attempts: 0,
        next_recovery_at: None,
    }
}

async fn probe_single_account(
    app: &AppHandle,
    state: &AppState,
    account: StoredAccount,
    _target_model: &str,
    client: &reqwest::Client,
    trigger: Option<&str>,
) -> (String, AccountFingerprintSummary) {
    let account_key = account.account_key();
    let total_samples = PROBE_SAMPLE_COUNT * 2;
    emit_progress(
        app,
        AccountFingerprintProgressEvent {
            account_key: account_key.clone(),
            status: "probing".to_string(),
            completed_samples: 0,
            total_samples,
            message: Some("准备授权...".to_string()),
            summary: None,
            api_proxy_enabled: Some(account.api_proxy_enabled),
            trigger: trigger.map(str::to_string),
        },
    );

    let mut auth_json = account_service::refresh_latest_auth_json_if_newer(
        app,
        state,
        &account_key,
        &account.auth_json,
    )
    .await;
    if auth::auth_tokens_need_refresh(&auth_json) {
        match account_service::refresh_account_auth_with_operation_guard(
            app,
            state,
            &account_key,
            &auth_json,
        )
        .await
        {
            Ok(refreshed) => auth_json = refreshed.auth_json,
            Err(failure) => {
                let err = failure
                    .refresh_error
                    .or(failure.auth_refresh_error)
                    .unwrap_or_else(|| "检测指纹前刷新授权失败".to_string());
                return (
                    account_key,
                    error_summary(DEFAULT_PROBE_MODEL, 0, None, err),
                );
            }
        }
    }

    let (sol_probe, updated_auth, fatal) = probe_single_model_for_account(
        app,
        state,
        &account_key,
        auth_json,
        client,
        PROBE_MODEL_SOL,
        "6.2sol",
        0,
        total_samples,
        trigger,
    )
    .await;

    let astra_probe = if let Some(fatal_err) = fatal {
        error_model_probe(PROBE_MODEL_ASTRA, 0, 0, None, fatal_err)
    } else if sol_probe.degraded && sol_probe.error.is_none() {
        // 6.2sol 已确认降智时直接早停，跳过 6.2astra 以节省高推理额度。
        skipped_model_probe(PROBE_MODEL_ASTRA)
    } else {
        let (astra, _, _) = probe_single_model_for_account(
            app,
            state,
            &account_key,
            updated_auth,
            client,
            PROBE_MODEL_ASTRA,
            "6.2astra",
            sol_probe.total_samples,
            sol_probe.total_samples + PROBE_SAMPLE_COUNT,
            trigger,
        )
        .await;
        astra
    };

    (account_key, compose_account_summary(sol_probe, astra_probe))
}

fn error_summary(
    target_model: &str,
    valid_samples: usize,
    ttft_ms: Option<u64>,
    err: String,
) -> AccountFingerprintSummary {
    AccountFingerprintSummary {
        degraded: false,
        target_model: target_model.to_string(),
        detected_model: String::new(),
        detected_model_id: String::new(),
        confidence: None,
        score: None,
        runner_up_model: None,
        runner_up_score: None,
        valid_samples,
        total_samples: PROBE_SAMPLE_COUNT * 2,
        ttft_ms,
        checked_at: now_unix_seconds(),
        error: Some(err.clone()),
        sol: Some(error_model_probe(
            PROBE_MODEL_SOL,
            0,
            PROBE_SAMPLE_COUNT,
            ttft_ms,
            err.clone(),
        )),
        astra: Some(error_model_probe(
            PROBE_MODEL_ASTRA,
            0,
            PROBE_SAMPLE_COUNT,
            ttft_ms,
            err,
        )),
        auto_disabled_by_fingerprint: false,
        auto_disabled_at: None,
        recovery_attempts: 0,
        next_recovery_at: None,
    }
}

fn account_has_blocking_auth_or_usage_issue(account: &StoredAccount) -> bool {
    account.auth_refresh_blocked
        || account.auth_refresh_error.is_some()
        || account.profile_integrity_error.is_some()
        || account.profile_last_validation_error.is_some()
        || account
            .usage_error
            .as_deref()
            .is_some_and(auto_switch::usage_error_is_authorization_failure)
}

fn quota_window_deferral(window: Option<&UsageWindow>, threshold: f64, now: i64) -> Option<i64> {
    let window = window?;
    if !window.used_percent.is_finite() || window.used_percent < threshold {
        return None;
    }
    let reset_at = window.reset_at.filter(|&ts| ts > now);
    Some(
        reset_at
            .map(|ts| ts.saturating_add(RECOVERY_WINDOW_RESET_LEAD_SECS))
            .unwrap_or_else(|| now.saturating_add(RECOVERY_BACKOFF_SECS[0])),
    )
}

fn quota_deferral_until(usage: Option<&UsageSnapshot>, now: i64) -> Option<i64> {
    let usage = usage?;
    let five_hour = quota_window_deferral(
        usage.five_hour.as_ref(),
        RECOVERY_MAX_FIVE_HOUR_USED_PERCENT,
        now,
    );
    let one_week = quota_window_deferral(
        usage.one_week.as_ref(),
        RECOVERY_MAX_ONE_WEEK_USED_PERCENT,
        now,
    );
    match (five_hour, one_week) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

fn window_reset_since(window: Option<&UsageWindow>, prev_checked_at: i64, now: i64) -> bool {
    let Some(window) = window else {
        return false;
    };
    let Some(reset_at) = window.reset_at else {
        return false;
    };
    if window.used_percent.is_finite()
        && window.used_percent >= RECOVERY_MAX_FIVE_HOUR_USED_PERCENT
    {
        return false;
    }
    if prev_checked_at < reset_at && reset_at.saturating_add(RECOVERY_WINDOW_RESET_LEAD_SECS) <= now
    {
        return true;
    }
    if window.window_seconds > 0 {
        let window_started_at = reset_at.saturating_sub(window.window_seconds);
        if prev_checked_at < window_started_at && window_started_at <= now {
            return true;
        }
    }
    false
}

fn window_reset_allows_early_recovery(
    prev_checked_at: i64,
    usage: Option<&UsageSnapshot>,
    now: i64,
) -> bool {
    if now.saturating_sub(prev_checked_at) < RECOVERY_MIN_INTERVAL_SECS {
        return false;
    }
    let Some(usage) = usage else {
        return false;
    };
    if quota_deferral_until(Some(usage), now).is_some() {
        return false;
    }
    window_reset_since(usage.five_hour.as_ref(), prev_checked_at, now)
        || window_reset_since(usage.one_week.as_ref(), prev_checked_at, now)
}

fn compute_next_recovery_at(now: i64, attempts: u32, usage: Option<&UsageSnapshot>) -> i64 {
    let idx = (attempts as usize).min(RECOVERY_BACKOFF_SECS.len() - 1);
    let mut next_at = now.saturating_add(RECOVERY_BACKOFF_SECS[idx]);
    if let Some(defer_until) = quota_deferral_until(usage, now) {
        next_at = next_at.max(defer_until);
    } else if let Some(five_hour) = usage.and_then(|u| u.five_hour.as_ref()) {
        // 5h 窗口接近耗尽时避开临界探测，等窗口重置后再测以节省残余额度。
        if five_hour.used_percent.is_finite()
            && five_hour.used_percent >= RECOVERY_PRE_RESET_DEFER_FIVE_HOUR_PERCENT
        {
            if let Some(reset_at) = five_hour.reset_at.filter(|&ts| ts > now && ts <= next_at) {
                next_at = next_at.max(reset_at.saturating_add(RECOVERY_WINDOW_RESET_LEAD_SECS));
            }
        }
    }
    next_at
}

fn compute_transient_recovery_retry_at(
    now: i64,
    error: Option<&str>,
    usage: Option<&UsageSnapshot>,
) -> i64 {
    let is_quota_exhausted = error.is_some_and(|raw| {
        let lower = raw.to_ascii_lowercase();
        lower.contains("usage_limit") || raw.contains("额度已耗尽")
    });
    let mut next_at = now.saturating_add(if is_quota_exhausted {
        RECOVERY_BACKOFF_SECS[0]
    } else {
        RECOVERY_TRANSIENT_RETRY_SECS
    });
    if let Some(defer_until) = quota_deferral_until(usage, now) {
        next_at = next_at.max(defer_until);
    } else if is_quota_exhausted {
        if let Some(reset_at) = usage
            .and_then(|u| u.five_hour.as_ref())
            .and_then(|w| w.reset_at)
            .filter(|&ts| ts > now)
        {
            next_at = next_at.max(reset_at.saturating_add(RECOVERY_WINDOW_RESET_LEAD_SECS));
        }
    }
    next_at
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryEligibility {
    Skip,
    DeferUntil(i64),
    Wait,
    Due,
}

fn recovery_probe_due(
    account: &StoredAccount,
    summary: &AccountFingerprintSummary,
    now: i64,
) -> RecoveryEligibility {
    if !matches!(account.source_kind, AccountSourceKind::Chatgpt)
        || account.api_proxy_enabled
        || !summary.auto_disabled_by_fingerprint
        || account_has_blocking_auth_or_usage_issue(account)
    {
        return RecoveryEligibility::Skip;
    }
    if let Some(defer_until) = quota_deferral_until(account.usage.as_ref(), now) {
        return RecoveryEligibility::DeferUntil(defer_until);
    }
    if window_reset_allows_early_recovery(summary.checked_at, account.usage.as_ref(), now) {
        return RecoveryEligibility::Due;
    }
    if summary.next_recovery_at.is_none_or(|ts| now >= ts) {
        return RecoveryEligibility::Due;
    }
    RecoveryEligibility::Wait
}

fn clear_recovery_schedule(summary: &mut AccountFingerprintSummary) {
    summary.auto_disabled_by_fingerprint = false;
    summary.auto_disabled_at = None;
    summary.recovery_attempts = 0;
    summary.next_recovery_at = None;
}

async fn persist_probe_outcome(
    app: &AppHandle,
    state: &AppState,
    account_key: &str,
    summary: &AccountFingerprintSummary,
) -> Result<(Option<bool>, AccountFingerprintSummary), String> {
    let (final_enabled, stored_summary, any_fresh_valid) = {
        let _guard = state.store_lock.lock().await;
        let mut store = load_store(app)?;
        let account_exists = store
            .accounts
            .iter()
            .any(|account| account.account_key() == account_key);
        if !account_exists {
            return Ok((None, summary.clone()));
        }

        let prev_summary = store.settings.account_fingerprints.get(account_key).cloned();
        let representative_usage = store
            .accounts
            .iter()
            .filter(|account| account.account_key() == account_key)
            .find_map(|account| account.usage.clone());

        let mut stored = merge_fingerprint_summary(prev_summary.as_ref(), summary);

        let any_fresh_valid = [summary.sol.as_ref(), summary.astra.as_ref()]
            .into_iter()
            .flatten()
            .any(|p| p.error.is_none() && !p.detected_model.is_empty())
            || (summary.error.is_none() && !summary.detected_model.is_empty());

        let now = summary.checked_at;
        let resolved_enabled = if any_fresh_valid && summary.degraded {
            for account in &mut store.accounts {
                if account.account_key() == account_key {
                    account.api_proxy_enabled = false;
                    account.updated_at = now;
                }
            }
            let was_auto_disabled = prev_summary
                .as_ref()
                .is_some_and(|prev| prev.auto_disabled_by_fingerprint);
            let next_attempts = if was_auto_disabled {
                prev_summary
                    .as_ref()
                    .map_or(1, |prev| prev.recovery_attempts.saturating_add(1))
            } else {
                0
            };
            stored.auto_disabled_by_fingerprint = true;
            stored.auto_disabled_at = prev_summary
                .as_ref()
                .and_then(|prev| prev.auto_disabled_at)
                .or(Some(now));
            stored.recovery_attempts = next_attempts;
            stored.next_recovery_at = Some(compute_next_recovery_at(
                now,
                next_attempts,
                representative_usage.as_ref(),
            ));
            Some(false)
        } else if any_fresh_valid && !summary.degraded {
            let was_auto_disabled = prev_summary
                .as_ref()
                .is_some_and(|prev| prev.auto_disabled_by_fingerprint);
            if was_auto_disabled {
                for account in &mut store.accounts {
                    if account.account_key() == account_key {
                        account.api_proxy_enabled = true;
                        account.updated_at = now;
                    }
                }
            }
            clear_recovery_schedule(&mut stored);
            store
                .accounts
                .iter()
                .find(|account| account.account_key() == account_key)
                .map(|account| account.api_proxy_enabled)
        } else {
            if let Some(prev) = prev_summary.as_ref() {
                stored.auto_disabled_by_fingerprint = prev.auto_disabled_by_fingerprint;
                stored.auto_disabled_at = prev.auto_disabled_at;
                stored.recovery_attempts = prev.recovery_attempts;
                stored.next_recovery_at = if prev.auto_disabled_by_fingerprint {
                    Some(compute_transient_recovery_retry_at(
                        now,
                        summary.error.as_deref(),
                        representative_usage.as_ref(),
                    ))
                } else {
                    prev.next_recovery_at
                };
            }
            store
                .accounts
                .iter()
                .find(|account| account.account_key() == account_key)
                .map(|account| account.api_proxy_enabled)
        };

        store
            .settings
            .account_fingerprints
            .insert(account_key.to_string(), stored.clone());
        save_store(app, &store)?;
        (resolved_enabled, stored, any_fresh_valid)
    };

    if final_enabled == Some(false) && any_fresh_valid && summary.degraded {
        let api_proxy = state.api_proxy.lock().await;
        if let Some(handle) = api_proxy.as_ref() {
            let mut snapshot = handle.shared.lock().await;
            if snapshot.active_account_key.as_deref() == Some(account_key) {
                snapshot.active_account_key = None;
                snapshot.active_account_id = None;
                snapshot.active_account_label = None;
                snapshot.sequential_account_key = None;
            }
            snapshot
                .sequential_session_affinity
                .retain(|_, affinity| affinity.account_key != account_key);
        }
    }

    Ok((final_enabled, stored_summary))
}

fn select_probe_targets(
    accounts: &[StoredAccount],
    requested_ids: Option<&[String]>,
) -> Vec<StoredAccount> {
    let mut seen_keys = std::collections::HashSet::new();
    let mut targets = Vec::new();
    for account in accounts {
        if matches!(account.source_kind, AccountSourceKind::Relay) {
            continue;
        }
        let key = account.account_key();
        if let Some(filter) = requested_ids.filter(|ids| !ids.is_empty()) {
            let matched = filter.iter().any(|item| item == &account.id || item == &key);
            if !matched {
                continue;
            }
        }
        if seen_keys.insert(key) {
            targets.push(account.clone());
        }
    }
    targets
}

pub(crate) async fn probe_account_fingerprints_internal(
    app: &AppHandle,
    state: &AppState,
    account_ids: Option<Vec<String>>,
    model: Option<String>,
) -> Result<ProbeAccountFingerprintsResult, String> {
    detector()?;
    let target_model = model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_PROBE_MODEL)
        .to_string();

    let targets = {
        let _guard = state.store_lock.lock().await;
        let store = load_store(app)?;
        select_probe_targets(&store.accounts, account_ids.as_deref())
    };
    if targets.is_empty() {
        return Err("没有可检测指纹的 ChatGPT 账号".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_ATTEMPT_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建指纹探测 HTTP 客户端失败: {e}"))?;

    let mut tasks = stream::iter(targets.into_iter().map(|account| {
        let target_model = target_model.clone();
        let client = client.clone();
        async move { probe_single_account(app, state, account, &target_model, &client, None).await }
    }))
    .buffer_unordered(PROBE_ACCOUNT_CONCURRENCY);

    while let Some((account_key, summary)) = tasks.next().await {
        let (final_enabled, stored_summary) =
            persist_probe_outcome(app, state, &account_key, &summary).await?;
        let probe_succeeded = summary.error.is_none();
        let confirmed_degraded = probe_succeeded && summary.degraded;
        sentinel_tracker_complete_probe(
            &account_key,
            stored_summary.checked_at,
            confirmed_degraded,
            probe_succeeded,
        );
        let status = if summary.error.is_some() {
            "error"
        } else {
            "done"
        };
        let total = stored_summary.total_samples.max(1);
        emit_progress(
            app,
            AccountFingerprintProgressEvent {
                account_key,
                status: status.to_string(),
                completed_samples: total,
                total_samples: total,
                message: summary.error.clone(),
                summary: Some(stored_summary),
                api_proxy_enabled: final_enabled,
                trigger: None,
            },
        );
    }

    let accounts = account_service::list_accounts_internal(app, state).await?;
    let account_fingerprints = {
        let _guard = state.store_lock.lock().await;
        load_store(app)?.settings.account_fingerprints
    };
    Ok(ProbeAccountFingerprintsResult {
        accounts,
        account_fingerprints,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentinelVerdict {
    Healthy,
    SuspiciousLowReasoning,
    ImmediateModelMismatch,
    Ignored,
}

#[derive(Default)]
struct SentinelTracker {
    streaks: HashMap<String, usize>,
    healthy_calls: HashMap<String, usize>,
    last_probed_at: HashMap<String, i64>,
    quarantined_until: HashMap<String, i64>,
    in_flight: HashSet<String>,
    waiters: HashMap<String, Vec<oneshot::Sender<bool>>>,
}

fn sentinel_tracker() -> &'static StdMutex<SentinelTracker> {
    static TRACKER: OnceLock<StdMutex<SentinelTracker>> = OnceLock::new();
    TRACKER.get_or_init(|| StdMutex::new(SentinelTracker::default()))
}

fn lock_sentinel_tracker() -> std::sync::MutexGuard<'static, SentinelTracker> {
    sentinel_tracker()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn is_account_temporarily_deprioritized(account_key: &str) -> bool {
    let now = now_unix_seconds();
    lock_sentinel_tracker().is_temporarily_deprioritized(account_key, now)
}

pub(crate) fn quarantine_account_on_mismatch(account_key: &str) {
    let now = now_unix_seconds();
    lock_sentinel_tracker().quarantine_on_mismatch(account_key, now);
}

pub(crate) fn clear_account_runtime_quarantine(account_key: &str) {
    lock_sentinel_tracker().clear_quarantine(account_key);
}

fn sentinel_tracker_complete_probe(
    account_key: &str,
    checked_at: i64,
    confirmed_degraded: bool,
    clear_quarantine: bool,
) {
    let waiters = lock_sentinel_tracker().mark_probe_completed(
        account_key,
        checked_at,
        confirmed_degraded,
        clear_quarantine,
    );
    for tx in waiters {
        let _ = tx.send(confirmed_degraded);
    }
}

fn is_heavy_reasoning_target(model: &str, effort: Option<&str>) -> bool {
    let Some(effort) = effort else {
        return false;
    };
    let effort_lower = effort.trim().to_ascii_lowercase();
    if !matches!(effort_lower.as_str(), "high" | "xhigh" | "max" | "ultra") {
        return false;
    }
    let model_lower = model.trim().to_ascii_lowercase();
    if model_lower.is_empty() {
        return false;
    }
    !["luna", "mini", "nano", "spark"]
        .iter()
        .any(|needle| model_lower.contains(needle))
}

fn classify_sentinel_observation(obs: &ProxySentinelObservation) -> SentinelVerdict {
    if let Some(resp_model) = obs.response_model.as_deref() {
        if proxy_service::is_upstream_response_model_degraded(&obs.model, resp_model) {
            return SentinelVerdict::ImmediateModelMismatch;
        }
    }
    if !is_heavy_reasoning_target(&obs.model, obs.reasoning_effort.as_deref()) {
        return SentinelVerdict::Ignored;
    }
    let Some(reasoning_tokens) = obs.reasoning_tokens else {
        return SentinelVerdict::Ignored;
    };
    if reasoning_tokens >= SENTINEL_HEALTHY_REASONING_THRESHOLD {
        return SentinelVerdict::Healthy;
    }
    if reasoning_tokens <= SENTINEL_LOW_REASONING_THRESHOLD
        && obs.input_tokens.unwrap_or(0) >= SENTINEL_MIN_INPUT_TOKENS
        && obs.output_tokens.unwrap_or(0) >= SENTINEL_MIN_OUTPUT_TOKENS
    {
        return SentinelVerdict::SuspiciousLowReasoning;
    }
    SentinelVerdict::Ignored
}

impl SentinelTracker {
    fn is_busy_or_cooling_down(&self, account_key: &str, now: i64) -> bool {
        self.in_flight.contains(account_key)
            || self
                .last_probed_at
                .get(account_key)
                .is_some_and(|&ts| now.saturating_sub(ts) < SENTINEL_COOLDOWN_SECS)
    }

    fn is_temporarily_deprioritized(&self, account_key: &str, now: i64) -> bool {
        self.in_flight.contains(account_key)
            || self
                .quarantined_until
                .get(account_key)
                .is_some_and(|&until| now < until)
    }

    fn quarantine_on_mismatch(&mut self, account_key: &str, now: i64) {
        let until = now.saturating_add(SENTINEL_MISMATCH_QUARANTINE_SECS);
        self.quarantined_until
            .entry(account_key.to_string())
            .and_modify(|prev| *prev = (*prev).max(until))
            .or_insert(until);
    }

    fn clear_quarantine(&mut self, account_key: &str) {
        self.quarantined_until.remove(account_key);
        self.streaks.insert(account_key.to_string(), 0);
        self.healthy_calls.insert(account_key.to_string(), 0);
    }

    fn push_waiter(&mut self, account_key: &str, waiter: Option<oneshot::Sender<bool>>) {
        if let Some(tx) = waiter {
            self.waiters
                .entry(account_key.to_string())
                .or_default()
                .push(tx);
        }
    }

    fn trigger_probe(&mut self, account_key: &str, now: i64, quarantine: bool) -> String {
        self.streaks.insert(account_key.to_string(), 0);
        self.healthy_calls.insert(account_key.to_string(), 0);
        self.last_probed_at.insert(account_key.to_string(), now);
        self.in_flight.insert(account_key.to_string());
        if quarantine {
            let until = now.saturating_add(SENTINEL_PROBING_QUARANTINE_SECS);
            self.quarantined_until
                .entry(account_key.to_string())
                .and_modify(|prev| *prev = (*prev).max(until))
                .or_insert(until);
        }
        account_key.to_string()
    }

    fn record_dispatch(
        &mut self,
        obs: &ProxySentinelObservation,
        waiter: Option<oneshot::Sender<bool>>,
        now: i64,
    ) -> Option<String> {
        match classify_sentinel_observation(obs) {
            SentinelVerdict::ImmediateModelMismatch => {
                self.quarantine_on_mismatch(&obs.account_key, now);
                if self.in_flight.contains(&obs.account_key) {
                    self.push_waiter(&obs.account_key, waiter);
                    return None;
                }
                if self.is_busy_or_cooling_down(&obs.account_key, now) {
                    if let Some(tx) = waiter {
                        let _ = tx.send(true);
                    }
                    return None;
                }
                self.push_waiter(&obs.account_key, waiter);
                Some(self.trigger_probe(&obs.account_key, now, true))
            }
            SentinelVerdict::Healthy => {
                self.streaks.insert(obs.account_key.clone(), 0);
                if let Some(tx) = waiter {
                    let _ = tx.send(false);
                }
                if self.is_busy_or_cooling_down(&obs.account_key, now) {
                    return None;
                }
                let next_healthy = self
                    .healthy_calls
                    .get(&obs.account_key)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1);
                if next_healthy < SENTINEL_AUDIT_HEALTHY_CALL_INTERVAL {
                    self.healthy_calls
                        .insert(obs.account_key.clone(), next_healthy);
                    return None;
                }
                Some(self.trigger_probe(&obs.account_key, now, false))
            }
            SentinelVerdict::Ignored => {
                if let Some(tx) = waiter {
                    let _ = tx.send(false);
                }
                None
            }
            SentinelVerdict::SuspiciousLowReasoning => {
                if self.in_flight.contains(&obs.account_key) {
                    self.push_waiter(&obs.account_key, waiter);
                    return None;
                }
                if self.is_busy_or_cooling_down(&obs.account_key, now) {
                    if let Some(tx) = waiter {
                        let _ = tx.send(false);
                    }
                    return None;
                }
                let next_streak = self
                    .streaks
                    .get(&obs.account_key)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1);
                if next_streak < SENTINEL_CONSECUTIVE_THRESHOLD {
                    self.streaks.insert(obs.account_key.clone(), next_streak);
                    if let Some(tx) = waiter {
                        let _ = tx.send(false);
                    }
                    return None;
                }
                self.push_waiter(&obs.account_key, waiter);
                Some(self.trigger_probe(&obs.account_key, now, true))
            }
        }
    }

    #[cfg(test)]
    fn record_observation(&mut self, obs: &ProxySentinelObservation, now: i64) -> Option<String> {
        self.record_dispatch(obs, None, now)
    }

    fn mark_probe_completed(
        &mut self,
        account_key: &str,
        checked_at: i64,
        _confirmed_degraded: bool,
        clear_quarantine: bool,
    ) -> Vec<oneshot::Sender<bool>> {
        self.streaks.insert(account_key.to_string(), 0);
        self.healthy_calls.insert(account_key.to_string(), 0);
        self.in_flight.remove(account_key);
        if clear_quarantine {
            self.quarantined_until.remove(account_key);
        }
        self.last_probed_at
            .entry(account_key.to_string())
            .and_modify(|prev| *prev = (*prev).max(checked_at))
            .or_insert(checked_at);
        self.waiters.remove(account_key).unwrap_or_default()
    }
}

pub(crate) fn spawn_proxy_sentinel(
    app: AppHandle,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ProxySentinelDispatch>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(dispatch) = rx.recv().await {
            let obs = dispatch.observation;
            let trigger_key = {
                let mut tracker = lock_sentinel_tracker();
                tracker.record_dispatch(&obs, dispatch.verification_tx, now_unix_seconds())
            };
            if let Some(account_key) = trigger_key {
                log::warn!(
                    "FINGERPRINT_SENTINEL action=trigger account={} model={} response_model={:?} effort={:?} reasoning_tokens={:?}",
                    account_key,
                    obs.model,
                    obs.response_model,
                    obs.reasoning_effort,
                    obs.reasoning_tokens
                );
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    run_sentinel_probe_for_account(app_clone, account_key).await;
                });
            }
        }
    });
}

async fn run_sentinel_probe_for_account(app: AppHandle, account_key: String) {
    let outcome = run_sentinel_probe_inner(&app, &account_key).await;
    let (confirmed_degraded, clear_quarantine) = match &outcome {
        Ok((degraded, clean)) => (*degraded, *clean),
        Err(_) => (false, false),
    };
    sentinel_tracker_complete_probe(
        &account_key,
        now_unix_seconds(),
        confirmed_degraded,
        clear_quarantine,
    );
    if let Err(err) = outcome {
        log::warn!(
            "FINGERPRINT_SENTINEL action=probe_failed account={} err={}",
            account_key,
            err
        );
    }
}

async fn run_sentinel_probe_inner(
    app: &AppHandle,
    account_key: &str,
) -> Result<(bool, bool), String> {
    detector()?;
    let state = app.state::<AppState>();
    let target = {
        let _guard = state.store_lock.lock().await;
        let store = load_store(app)?;
        let now = now_unix_seconds();
        if let Some(prev) = store.settings.account_fingerprints.get(account_key) {
            if now.saturating_sub(prev.checked_at) < SENTINEL_COOLDOWN_SECS {
                return Ok((prev.degraded, !prev.degraded));
            }
        }
        store
            .accounts
            .into_iter()
            .find(|acc| {
                acc.account_key() == account_key
                    && matches!(acc.source_kind, AccountSourceKind::Chatgpt)
                    && acc.api_proxy_enabled
            })
    };
    let Some(account) = target else {
        return Ok((false, true));
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_ATTEMPT_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("创建哨兵探测 HTTP 客户端失败: {e}"))?;

    let (_, summary) = probe_single_account(
        app,
        state.inner(),
        account,
        DEFAULT_PROBE_MODEL,
        &client,
        Some("sentinel"),
    )
    .await;
    let (final_enabled, stored_summary) =
        persist_probe_outcome(app, state.inner(), account_key, &summary).await?;
    let _ = tray::refresh_usage_surfaces_snapshot(app);

    let probe_clean = summary.error.is_none();
    let confirmed_degraded = probe_clean && summary.degraded;
    let status = if summary.error.is_some() && !summary.degraded {
        "error"
    } else {
        "done"
    };
    let total = stored_summary.total_samples.max(1);
    emit_progress(
        app,
        AccountFingerprintProgressEvent {
            account_key: account_key.to_string(),
            status: status.to_string(),
            completed_samples: total,
            total_samples: total,
            message: summary.error.clone(),
            summary: Some(stored_summary),
            api_proxy_enabled: final_enabled,
            trigger: Some("sentinel".to_string()),
        },
    );
    Ok((confirmed_degraded, probe_clean))
}

static RECOVERY_CYCLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct RecoveryCycleGuard;
impl Drop for RecoveryCycleGuard {
    fn drop(&mut self) {
        RECOVERY_CYCLE_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn spawn_recovery_probes_if_due(app: AppHandle) {
    if RECOVERY_CYCLE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _guard = RecoveryCycleGuard;
        let state = app.state::<AppState>();
        if let Err(err) = run_due_recovery_probes_internal(&app, state.inner()).await {
            log::warn!("FINGERPRINT_RECOVERY action=cycle_failed err={}", err);
        }
    });
}

async fn run_due_recovery_probes_internal(
    app: &AppHandle,
    state: &AppState,
) -> Result<(), String> {
    detector()?;
    let now = now_unix_seconds();

    let due_targets = {
        let _guard = state.store_lock.lock().await;
        let mut store = load_store(app)?;
        let mut store_dirty = false;
        let mut due_candidates: Vec<(i64, StoredAccount)> = Vec::new();
        let tracked_keys: Vec<String> = store
            .settings
            .account_fingerprints
            .iter()
            .filter(|(_, fp)| fp.auto_disabled_by_fingerprint)
            .map(|(k, _)| k.clone())
            .collect();

        for account_key in tracked_keys {
            let matching: Vec<StoredAccount> = store
                .accounts
                .iter()
                .filter(|acc| acc.account_key() == account_key)
                .cloned()
                .collect();
            if matching.is_empty() {
                continue;
            }
            if matching.iter().all(|acc| acc.api_proxy_enabled) {
                if let Some(fp) = store.settings.account_fingerprints.get_mut(&account_key) {
                    clear_recovery_schedule(fp);
                    store_dirty = true;
                }
                continue;
            }
            let Some(rep) = matching
                .iter()
                .find(|acc| matches!(acc.source_kind, AccountSourceKind::Chatgpt) && !acc.api_proxy_enabled)
                .cloned()
            else {
                continue;
            };
            let Some(summary) = store.settings.account_fingerprints.get(&account_key).cloned()
            else {
                continue;
            };

            match recovery_probe_due(&rep, &summary, now) {
                RecoveryEligibility::Due => {
                    let sort_ts = summary.next_recovery_at.unwrap_or(summary.checked_at);
                    due_candidates.push((sort_ts, rep));
                }
                RecoveryEligibility::DeferUntil(defer_until) => {
                    if summary
                        .next_recovery_at
                        .is_none_or(|prev| defer_until > prev)
                    {
                        if let Some(fp) = store.settings.account_fingerprints.get_mut(&account_key) {
                            fp.next_recovery_at = Some(defer_until);
                            store_dirty = true;
                        }
                    }
                }
                RecoveryEligibility::Skip | RecoveryEligibility::Wait => {}
            }
        }

        if store_dirty {
            save_store(app, &store)?;
        }

        due_candidates.sort_by_key(|(ts, acc)| (*ts, acc.account_key()));
        due_candidates
            .into_iter()
            .map(|(_, acc)| acc)
            .collect::<Vec<_>>()
    };

    if due_targets.is_empty() {
        return Ok(());
    }

    let mut chosen: Option<StoredAccount> = None;
    for candidate in due_targets {
        let key = candidate.account_key();
        let mut tracker = lock_sentinel_tracker();
        if !tracker.in_flight.contains(&key) {
            tracker.in_flight.insert(key);
            chosen = Some(candidate);
            break;
        }
    }
    let Some(account) = chosen else {
        return Ok(());
    };
    let account_key = account.account_key();

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_ATTEMPT_TIMEOUT_SECS))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            sentinel_tracker_complete_probe(&account_key, now_unix_seconds(), false, false);
            return Err(format!("创建指纹恢复探测 HTTP 客户端失败: {err}"));
        }
    };

    let mut probed = 0usize;
    if probed < RECOVERY_MAX_PROBES_PER_CYCLE {
        probed += 1;
        let _ = probed;
        log::info!(
            "FINGERPRINT_RECOVERY action=start account={}",
            account_key
        );
        let (_, summary) = probe_single_account(
            app,
            state,
            account,
            DEFAULT_PROBE_MODEL,
            &client,
            Some("recovery"),
        )
        .await;
        let persist_res = persist_probe_outcome(app, state, &account_key, &summary).await;
        let probe_clean = summary.error.is_none();
        let confirmed_degraded = probe_clean && summary.degraded;
        sentinel_tracker_complete_probe(
            &account_key,
            summary.checked_at,
            confirmed_degraded,
            probe_clean,
        );
        let (final_enabled, stored_summary) = persist_res?;
        let _ = tray::refresh_usage_surfaces_snapshot(app);
        log::info!(
            "FINGERPRINT_RECOVERY action=done account={} degraded={} enabled={:?} next_recovery_at={:?}",
            account_key,
            stored_summary.degraded,
            final_enabled,
            stored_summary.next_recovery_at
        );

        let status = if summary.error.is_some() && !summary.degraded {
            "error"
        } else {
            "done"
        };
        let total = stored_summary.total_samples.max(1);
        emit_progress(
            app,
            AccountFingerprintProgressEvent {
                account_key,
                status: status.to_string(),
                completed_samples: total,
                total_samples: total,
                message: summary.error.clone(),
                summary: Some(stored_summary),
                api_proxy_enabled: final_enabled,
                trigger: Some("recovery".to_string()),
            },
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCT2_R1_SAMPLES: [(&str, usize); 3] = [
        ("247, 83, 319, 46, 172, 294, 11, 208, 135, 351, 64, 226, 157, 302, 91, 18, 273, 340, 119, 185, 332, 53, 214, 98, 261, 7, 146, 308, 177, 235, 124, 289, 39, 354, 163, 76, 221, 14, 297, 108, 193, 31, 267, 142, 325, 59, 201, 87, 248, 16, 281, 113, 344, 68, 230, 151, 5, 316, 179, 94, 42, 255, 129, 337, 196, 22, 284, 73, 218, 160, 305, 101, 49, 242, 188, 353, 132, 26, 270, 81, 166, 321, 57, 211, 6, 293, 145, 234, 104, 349, 89, 276, 181, 35, 312, 121, 203, 66, 250, 154, 328, 13, 224, 97, 287, 174, 44, 339, 115, 259, 71, 199, 346, 138, 24, 265, 182, 309, 52, 217, 149, 334, 85, 237, 9, 291, 168, 106, 320, 62, 278, 127, 47, 205, 355, 158, 33, 244, 92, 301, 191, 56, 263, 117, 342, 79, 228, 4, 286, 153, 103, 327, 176, 28, 252, 140, 314, 69, 210, 86, 239, 12, 298, 162, 48, 331, 123, 280, 197, 74, 347, 134, 223, 37, 304, 96, 256, 171, 20, 318, 61, 187, 274, 111, 329, 45, 231, 150, 8, 295, 213, 82, 341, 156, 29, 266, 120, 307, 194, 54, 285, 17, 233, 99, 352, 144, 67, 260, 183, 322, 109, 246, 41, 290, 164, 3, 336, 202, 77, 225, 152, 311, 93, 271, 36, 198, 348, 126, 58, 240, 324, 178, 15, 254, 102, 283, 147, 70, 216, 333, 32, 190, 300, 84, 262, 137, 21, 229, 345, 165, 277, 114, 51, 206, 317, 88, 249, 180, 10, 296, 143, 335, 63, 219, 27, 288, 170, 107, 350, 236, 75, 253, 130, 310, 43, 195, 269, 118, 323, 2, 204, 95, 282, 155, 38, 343, 227, 65, 299, 173, 25, 241, 112, 326, 186, 55, 275, 139, 212, 80, 306, 161, 23", 303),
        ("247, 63, 319, 108, 14, 286, 175, 92, 331, 46, 201, 129, 354, 77, 218, 163, 28, 294, 117, 340, 56, 182, 309, 85, 226, 151, 7, 273, 194, 39, 322, 136, 98, 251, 67, 305, 142, 21, 233, 178, 347, 113, 264, 49, 188, 316, 81, 205, 159, 34, 281, 124, 352, 96, 240, 11, 302, 173, 58, 215, 138, 327, 72, 269, 43, 197, 335, 104, 253, 17, 166, 290, 121, 344, 89, 229, 61, 311, 148, 24, 276, 185, 52, 333, 109, 208, 3, 257, 154, 296, 79, 234, 131, 350, 42, 181, 320, 95, 261, 13, 203, 145, 287, 68, 172, 37, 313, 119, 246, 84, 355, 222, 54, 284, 161, 9, 238, 102, 337, 193, 268, 126, 47, 304, 156, 213, 75, 341, 19, 231, 178, 298, 112, 65, 325, 140, 249, 87, 16, 206, 332, 123, 279, 50, 191, 308, 73, 244, 132, 26, 292, 169, 348, 106, 221, 59, 267, 150, 31, 314, 83, 236, 174, 353, 115, 288, 44, 199, 258, 12, 329, 147, 91, 211, 301, 66, 183, 277, 38, 243, 118, 346, 157, 5, 224, 99, 310, 71, 260, 137, 23, 295, 162, 339, 55, 202, 283, 111, 248, 80, 318, 143, 29, 230, 97, 271, 186, 350, 62, 216, 153, 306, 8, 252, 127, 334, 177, 41, 289, 103, 241, 69, 323, 195, 15, 266, 120, 343, 88, 209, 158, 297, 232, 53, 180, 315, 134, 25, 274, 101, 345, 164, 76, 255, 33, 219, 300, 146, 93, 282, 207, 18, 330, 114, 263, 171, 48, 237, 125, 307, 190, 64, 351, 228, 280, 139, 2, 198, 324, 86, 250, 167, 110, 293, 57, 214, 342, 155, 30, 270, 204, 82, 317, 133, 245, 10, 291, 176, 336, 122, 70, 259, 187, 45, 225, 303, 149, 278, 94, 35, 321, 212, 116, 254, 60, 196, 349, 160, 27, 239, 285, 107, 262, 74, 184, 312, 141, 6, 227, 338, 100, 272, 51, 200, 326, 165, 90, 235", 328),
        ("217, 43, 308, 126, 7, 274, 159, 332, 85, 191, 54, 346, 112, 263, 19, 228, 301, 67, 143, 320, 178, 31, 249, 95, 354, 138, 206, 73, 287, 164, 12, 235, 104, 319, 48, 183, 271, 151, 339, 82, 293, 24, 196, 117, 257, 61, 325, 146, 9, 211, 168, 352, 39, 282, 123, 76, 244, 17, 310, 202, 98, 267, 145, 334, 52, 221, 181, 3, 298, 114, 253, 69, 327, 132, 22, 286, 174, 93, 341, 238, 57, 208, 149, 316, 35, 261, 187, 106, 348, 80, 230, 14, 303, 162, 45, 279, 119, 337, 198, 64, 155, 291, 28, 216, 101, 350, 242, 71, 184, 314, 47, 269, 136, 6, 225, 322, 89, 172, 255, 109, 343, 58, 193, 277, 34, 152, 307, 125, 233, 16, 284, 97, 330, 204, 41, 166, 251, 78, 355, 142, 218, 26, 295, 111, 264, 53, 177, 324, 90, 239, 130, 347, 65, 200, 11, 288, 157, 305, 84, 223, 49, 273, 118, 336, 190, 32, 246, 102, 313, 170, 75, 259, 20, 329, 140, 214, 92, 283, 162, 37, 299, 121, 234, 8, 345, 180, 63, 268, 147, 315, 55, 207, 99, 276, 25, 154, 340, 227, 113, 289, 72, 195, 321, 46, 250, 134, 18, 304, 86, 241, 173, 353, 60, 212, 128, 280, 40, 186, 333, 105, 258, 5, 144, 296, 79, 220, 167, 311, 51, 237, 94, 349, 122, 266, 30, 201, 156, 318, 68, 247, 110, 281, 44, 194, 326, 83, 229, 13, 165, 302, 139, 270, 56, 210, 96, 342, 175, 23, 254, 115, 292, 70, 188, 335, 38, 224, 150, 262, 88, 317, 10, 205, 133, 278, 59, 243, 171, 351, 100, 29, 232, 160, 309, 77, 199, 344, 42, 256, 120, 285, 66, 215, 148, 331, 21", 295),
    ];

    const ACCT8_R1_SAMPLES: [(&str, usize); 3] = [
        ("247, 19, 318, 74, 156, 291, 43, 225, 107, 352, 68, 184, 13, 276, 331, 96, 204, 51, 142, 263, 8, 347, 121, 238, 59, 312, 177, 35, 269, 89, 214, 153, 326, 22, 198, 344, 116, 281, 63, 235, 302, 47, 171, 309, 134, 257, 77, 349, 162, 28, 193, 86, 244, 11, 333, 148, 219, 55, 287, 101, 327, 39, 173, 252, 5, 316, 129, 221, 66, 274, 98, 345, 182, 30, 262, 118, 206, 337, 72, 159, 285, 24, 190, 354, 109, 233, 48, 299, 137, 81, 172, 315, 61, 246, 14, 226, 340, 103, 279, 157, 36, 288, 201, 123, 348, 87, 164, 308, 53, 237, 319, 26, 145, 271, 94, 332, 188, 7, 258, 215, 112, 302, 41, 196, 328, 75, 249, 131, 343, 18, 224, 57, 286, 120, 310, 92, 169, 334, 45, 203, 83, 278, 151, 346, 32, 217, 138, 267, 99, 175, 6, 242, 314, 114, 231, 65, 289, 160, 350, 127, 212, 52, 327, 136, 293, 80, 185, 241, 17, 306, 122, 254, 70, 339, 147, 272, 29, 198, 105, 317, 58, 229, 341, 93, 161, 283, 37, 240, 126, 355, 216, 49, 273, 10, 187, 325, 108, 260, 143, 304, 90, 232, 67, 311, 176, 27, 248, 135, 342, 154, 223, 4, 294, 115, 280, 46, 165, 313, 102, 234, 338, 71, 192, 25, 266, 130, 307, 54, 218, 146, 88, 250, 330, 20, 179, 264, 113, 300, 34, 205, 319, 62, 228, 144, 91, 336, 156, 76, 270, 183, 12, 290, 110, 239, 333, 56, 170, 282, 100, 323, 42, 197, 247, 69, 155, 347, 216, 84, 275, 128, 178, 31, 309, 117, 243, 353, 50, 222, 139, 268, 104, 199, 73, 324, 163, 201, 286, 21, 181, 340, 289, 44, 132, 208, 95, 326, 60, 236", 298),
        ("247, 18, 319, 76, 142, 291, 53, 208, 334, 97, 11, 265, 184, 350, 61, 229, 128, 303, 44, 172, 286, 7, 156, 337, 89, 214, 24, 278, 115, 194, 326, 68, 251, 38, 147, 309, 102, 220, 5, 182, 271, 131, 346, 57, 236, 84, 163, 298, 31, 205, 119, 352, 73, 284, 16, 244, 190, 126, 315, 48, 207, 94, 333, 139, 267, 22, 177, 306, 81, 258, 63, 146, 320, 3, 231, 108, 273, 165, 349, 35, 201, 87, 292, 154, 12, 246, 328, 116, 59, 187, 275, 42, 223, 101, 317, 69, 134, 255, 8, 296, 158, 340, 27, 213, 122, 279, 49, 169, 308, 92, 234, 14, 263, 193, 326, 75, 219, 137, 354, 56, 181, 312, 29, 145, 270, 99, 238, 60, 330, 164, 21, 283, 114, 248, 304, 39, 202, 132, 345, 86, 216, 67, 289, 153, 10, 174, 321, 47, 260, 123, 338, 78, 226, 185, 52, 276, 109, 300, 33, 242, 117, 196, 314, 6, 161, 266, 88, 235, 348, 71, 204, 43, 125, 307, 159, 282, 36, 195, 98, 327, 15, 254, 188, 324, 65, 141, 278, 110, 232, 46, 293, 83, 136, 311, 9, 239, 171, 350, 58, 217, 104, 281, 26, 199, 329, 74, 168, 290, 121, 247, 41, 316, 127, 233, 180, 85, 344, 54, 269, 160, 222, 13, 297, 155, 318, 34, 243, 140, 228, 82, 335, 192, 107, 280, 62, 173, 305, 25, 252, 129, 191, 347, 79, 218, 4, 287, 130, 261, 103, 237, 166, 322, 50, 234, 306, 19, 249, 90, 175, 332, 118, 268, 37, 210, 86, 301, 23, 157, 225, 343, 72, 215, 135, 288, 30, 198, 113, 327, 64, 240, 106, 277, 162, 45, 294, 138, 170, 319, 80, 212, 35, 189, 272, 100, 325", 295),
        ("247, 18, 331, 92, 156, 4, 283, 119, 344, 61, 205, 137, 298, 73, 226, 11, 184, 315, 48\n\n102, 269, 34, 352, 167, 88, 241, 126, 7, 293, 54, 218, 145, 326, 79, 190, 3, 260, 114\n\n335, 28, 174, 249, 96, 312, 43, 221, 132, 350, 68, 155, 207, 15, 287, 109, 234, 57, 193\n\n81, 276, 122, 346, 39, 164, 299, 8, 216, 141, 327, 52, 186, 245, 117, 333, 71, 202, 24\n\n304, 91, 158, 12, 271, 129, 347, 63, 228, 104, 181, 316, 46, 239, 75, 290, 137, 21, 198\n\n351, 84, 166, 309, 31, 223, 112, 257, 5, 143, 295, 62, 178, 341, 97, 232, 19, 268, 120\n\n324, 49, 153, 278, 87, 200, 14, 337, 115, 242, 69, 306, 134, 189, 36, 264, 101, 220, 58\n\n285, 9, 176, 314, 44, 238, 127, 349, 77, 161, 292, 25, 203, 146, 332, 93, 214, 67, 247\n\n172, 328, 53, 260, 118, 301, 22, 183, 343, 89, 231, 108, 274, 40, 158, 315, 6, 196, 248\n\n64, 289, 135, 350, 76, 225, 17, 307, 154, 99, 236, 279, 33, 185, 121, 342, 47, 263, 210\n\n26, 319, 145, 294, 83, 173, 11, 327, 138, 240, 55, 302, 190, 72, 217, 345, 107, 234, 160\n\n299, 38, 188, 316, 124, 252, 68, 27, 340, 95, 206, 131, 286, 16, 229, 357, 74, 165, 311\n\n58, 243, 113, 334, 20, 180, 270, 86, 147, 322, 45, 201, 136, 278, 10, 256, 169, 305, 37\n\n221, 98, 348, 152, 61, 291, 123, 175, 13, 237, 330, 70, 194, 282, 105, 318, 42, 227, 159\n\n344, 30, 187, 273, 116, 254, 82, 309, 144, 215, 50, 296, 66, 177, 339, 23, 128, 246, 103\n\n275, 90, 212, 35, 320, 140, 265, 59, 333, 110, 192, 280, 41, 204, 300, 78, 235, 100, 338", 304),
    ];

    #[test]
    fn detector_decodes_and_matches_real_normal_and_degraded_samples() {
        let normal_inputs: Vec<OutputSample> = ACCT2_R1_SAMPLES
            .iter()
            .map(|(text, expected_count)| OutputSample {
                text: (*text).to_string(),
                expected_count: *expected_count,
            })
            .collect();
        let normal = analyze_outputs(&normal_inputs, true).expect("analyze #2");
        assert_eq!(normal.decision, "scored");
        assert_eq!(normal.used_outputs, 3);
        let top = &normal.results[0];
        assert_eq!(top.model, "gpt-6-astra");
        assert_eq!(top.display_name, "gpt-6-astra/6.1-sol");
        assert!((top.score - 3.5635600078508465).abs() < 1e-9);
        assert!((top.probability.unwrap() - 0.9990573482426603).abs() < 1e-9);
        assert!(!is_model_degraded(
            "gpt-6.1-sol",
            &top.model,
            &top.display_name
        ));
        assert!(!is_model_degraded(
            PROBE_MODEL_SOL,
            "gpt-6.2-sol",
            "gpt-6.2-sol"
        ));
        assert!(is_model_degraded(
            PROBE_MODEL_SOL,
            "gpt-5.6-luna",
            "gpt-5.6-luna"
        ));
        assert!(!is_model_degraded(
            PROBE_MODEL_ASTRA,
            "gpt-6.2-astra",
            "gpt-6.2-astra"
        ));
        assert!(is_model_degraded(
            PROBE_MODEL_ASTRA,
            "gpt-6-sol",
            "gpt-6-sol"
        ));

        let degraded_inputs: Vec<OutputSample> = ACCT8_R1_SAMPLES
            .iter()
            .map(|(text, expected_count)| OutputSample {
                text: (*text).to_string(),
                expected_count: *expected_count,
            })
            .collect();
        let degraded = analyze_outputs(&degraded_inputs, true).expect("analyze #8");
        assert_eq!(degraded.decision, "scored");
        let top_deg = &degraded.results[0];
        assert_eq!(top_deg.model, "gpt-5.6-luna");
        assert!((top_deg.score - 4.575956923790084).abs() < 1e-9);
        assert!((top_deg.probability.unwrap() - 0.9999967194091328).abs() < 1e-9);
        assert!(is_model_degraded(
            "gpt-6.1-sol",
            &top_deg.model,
            &top_deg.display_name
        ));
    }

    #[test]
    fn generates_distinct_challenge_lengths() {
        let challenges = generate_challenges(3);
        assert_eq!(challenges.len(), 3);
        assert_ne!(challenges[0].expected_count, challenges[1].expected_count);
        assert_ne!(challenges[1].expected_count, challenges[2].expected_count);
        assert!(challenges
            .iter()
            .all(|c| (292..=332).contains(&c.expected_count) && !c.prompt.is_empty()));
    }

    #[test]
    fn failed_reprobe_preserves_previous_detected_model_and_formats_quota_limit() {
        let msg = format_upstream_error(
            "上游拒绝 HTTP 429",
            r#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached"}}"#,
        );
        assert!(msg.contains("usage_limit_reached"));

        let prev_sol = ModelFingerprintProbe {
            target_model: PROBE_MODEL_SOL.to_string(),
            effort: PROBE_REASONING_EFFORT.to_string(),
            degraded: true,
            skipped: false,
            detected_model: "gpt-5.6-luna".to_string(),
            detected_model_id: "gpt-5.6-luna".to_string(),
            confidence: Some(0.9999),
            score: Some(4.5),
            runner_up_model: Some("gpt-6-luna".to_string()),
            runner_up_score: Some(2.2),
            valid_samples: 3,
            total_samples: 3,
            ttft_ms: Some(12350),
            error: None,
        };
        let prev = AccountFingerprintSummary {
            degraded: true,
            target_model: DEFAULT_PROBE_MODEL.to_string(),
            detected_model: "gpt-5.6-luna".to_string(),
            detected_model_id: "gpt-5.6-luna".to_string(),
            confidence: Some(0.9999),
            score: Some(4.5),
            runner_up_model: Some("gpt-6-luna".to_string()),
            runner_up_score: Some(2.2),
            valid_samples: 6,
            total_samples: 6,
            ttft_ms: Some(12350),
            checked_at: 1795079244,
            error: None,
            sol: Some(prev_sol.clone()),
            astra: Some(ModelFingerprintProbe {
                target_model: PROBE_MODEL_ASTRA.to_string(),
                ..prev_sol
            }),
            auto_disabled_by_fingerprint: false,
            auto_disabled_at: None,
            recovery_attempts: 0,
            next_recovery_at: None,
        };
        let next_err = error_summary(DEFAULT_PROBE_MODEL, 0, None, msg.clone());
        let merged = merge_fingerprint_summary(Some(&prev), &next_err);
        assert!(merged.degraded);
        assert_eq!(merged.detected_model, "gpt-5.6-luna");
        assert_eq!(merged.sol.as_ref().unwrap().detected_model, "gpt-5.6-luna");
        assert_eq!(merged.astra.as_ref().unwrap().detected_model, "gpt-5.6-luna");
        assert_eq!(merged.error.as_deref(), Some(msg.as_str()));
    }

    #[test]
    fn adaptive_two_sample_fast_path_accepts_clear_real_pairs_and_rejects_incomplete() {
        let normal_two: Vec<OutputSample> = ACCT2_R1_SAMPLES[..2]
            .iter()
            .map(|(text, expected_count)| OutputSample {
                text: (*text).to_string(),
                expected_count: *expected_count,
            })
            .collect();
        assert!(should_early_accept_two_samples(&normal_two));

        let degraded_two: Vec<OutputSample> = ACCT8_R1_SAMPLES[..2]
            .iter()
            .map(|(text, expected_count)| OutputSample {
                text: (*text).to_string(),
                expected_count: *expected_count,
            })
            .collect();
        assert!(should_early_accept_two_samples(&degraded_two));

        let one_empty = vec![
            normal_two[0].clone(),
            OutputSample {
                text: String::new(),
                expected_count: 300,
            },
        ];
        assert!(!should_early_accept_two_samples(&one_empty));
        assert!(!should_early_accept_two_samples(&normal_two[..1]));
    }

    #[test]
    fn skipped_astra_replaces_stale_previous_probe_and_composes_cleanly() {
        let prev_sol = ModelFingerprintProbe {
            target_model: PROBE_MODEL_SOL.to_string(),
            effort: PROBE_REASONING_EFFORT.to_string(),
            degraded: false,
            skipped: false,
            detected_model: "gpt-6.2-sol".to_string(),
            detected_model_id: "gpt-6.2-sol".to_string(),
            confidence: Some(0.99),
            score: Some(3.8),
            runner_up_model: None,
            runner_up_score: None,
            valid_samples: 3,
            total_samples: 3,
            ttft_ms: Some(2100),
            error: None,
        };
        let prev = AccountFingerprintSummary {
            degraded: false,
            target_model: DEFAULT_PROBE_MODEL.to_string(),
            detected_model: "gpt-6.2-sol / gpt-6.2-astra".to_string(),
            detected_model_id: "gpt-6.2-sol".to_string(),
            confidence: Some(0.99),
            score: Some(3.8),
            runner_up_model: None,
            runner_up_score: None,
            valid_samples: 6,
            total_samples: 6,
            ttft_ms: Some(2100),
            checked_at: 1795000000,
            error: None,
            sol: Some(prev_sol.clone()),
            astra: Some(ModelFingerprintProbe {
                target_model: PROBE_MODEL_ASTRA.to_string(),
                detected_model: "gpt-6.2-astra".to_string(),
                detected_model_id: "gpt-6.2-astra".to_string(),
                ..prev_sol
            }),
            auto_disabled_by_fingerprint: false,
            auto_disabled_at: None,
            recovery_attempts: 0,
            next_recovery_at: None,
        };

        let next_sol = ModelFingerprintProbe {
            target_model: PROBE_MODEL_SOL.to_string(),
            effort: PROBE_REASONING_EFFORT.to_string(),
            degraded: true,
            skipped: false,
            detected_model: "gpt-5.6-luna".to_string(),
            detected_model_id: "gpt-5.6-luna".to_string(),
            confidence: None,
            score: Some(4.4),
            runner_up_model: Some("gpt-6-luna".to_string()),
            runner_up_score: Some(1.9),
            valid_samples: 2,
            total_samples: 2,
            ttft_ms: Some(1800),
            error: None,
        };
        let early_stop_summary =
            compose_account_summary(next_sol, skipped_model_probe(PROBE_MODEL_ASTRA));
        assert!(early_stop_summary.degraded);
        assert_eq!(early_stop_summary.detected_model, "gpt-5.6-luna");
        assert_eq!(early_stop_summary.total_samples, 2);
        assert_eq!(early_stop_summary.valid_samples, 2);

        let merged = merge_fingerprint_summary(Some(&prev), &early_stop_summary);
        assert!(merged.degraded);
        assert_eq!(merged.detected_model, "gpt-5.6-luna");
        assert!(merged.astra.as_ref().unwrap().skipped);
        assert_eq!(merged.astra.as_ref().unwrap().total_samples, 0);
    }

    #[test]
    fn sentinel_tracker_triggers_on_consecutive_low_reasoning_and_respects_cooldown() {
        let mut tracker = SentinelTracker::default();
        let make_obs = |model: &str, effort: Option<&str>, reasoning_tokens: Option<i64>| {
            ProxySentinelObservation {
                account_key: "user@example.com|acc1".to_string(),
                model: model.to_string(),
                response_model: None,
                reasoning_effort: effort.map(str::to_string),
                input_tokens: Some(1600),
                output_tokens: Some(180),
                reasoning_tokens,
            }
        };

        // Low effort or mini model must never advance the streak.
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("low"), Some(0)), 100),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-5.6-luna", Some("xhigh"), Some(0)), 101),
            None
        );

        // Two suspicious heavy calls build streak without triggering yet; a healthy call resets it.
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(4)), 102),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("high"), Some(0)), 103),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(240)), 104),
            None
        );

        // Three consecutive suspicious heavy calls trigger once.
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)), 200),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(8)), 201),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-astra", Some("xhigh"), Some(0)), 202),
            Some("user@example.com|acc1".to_string())
        );

        // While in_flight or inside 15-minute cooldown, additional anomalies do not retrigger.
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)), 210),
            None
        );
        tracker.mark_probe_completed("user@example.com|acc1", 215, false, true);
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)), 300),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)), 301),
            None
        );
        assert_eq!(
            tracker.record_observation(&make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)), 302),
            None
        );

        // After cooldown expires, three consecutive anomalies trigger again.
        let after_cooldown = 215 + SENTINEL_COOLDOWN_SECS + 10;
        assert_eq!(
            tracker.record_observation(
                &make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)),
                after_cooldown
            ),
            None
        );
        assert_eq!(
            tracker.record_observation(
                &make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)),
                after_cooldown + 1
            ),
            None
        );
        assert_eq!(
            tracker.record_observation(
                &make_obs("gpt-6.2-sol", Some("xhigh"), Some(0)),
                after_cooldown + 2
            ),
            Some("user@example.com|acc1".to_string())
        );
    }

    #[test]
    fn sentinel_tracker_supports_max_ultra_mismatch_and_periodic_audit() {
        let mut tracker = SentinelTracker::default();
        let make_obs = |acc: &str,
                        model: &str,
                        response_model: Option<&str>,
                        effort: Option<&str>,
                        reasoning_tokens: Option<i64>| ProxySentinelObservation {
            account_key: acc.to_string(),
            model: model.to_string(),
            response_model: response_model.map(str::to_string),
            reasoning_effort: effort.map(str::to_string),
            input_tokens: Some(1600),
            output_tokens: Some(180),
            reasoning_tokens,
        };

        // max and ultra efforts count toward heavy-reasoning streak.
        assert_eq!(
            tracker.record_observation(
                &make_obs("acc-ultra", "gpt-6.2-astra", None, Some("max"), Some(0)),
                100
            ),
            None
        );
        assert_eq!(
            tracker.record_observation(
                &make_obs("acc-ultra", "gpt-6.2-astra", None, Some("ultra"), Some(4)),
                101
            ),
            None
        );
        assert_eq!(
            tracker.record_observation(
                &make_obs("acc-ultra", "gpt-6.2-astra", None, Some("max"), Some(8)),
                102
            ),
            Some("acc-ultra".to_string())
        );

        // Response model mismatch triggers immediately even on low effort; dated snapshot on 6.1-sol does not false-positive.
        assert!(!proxy_service::is_upstream_response_model_degraded(
            "gpt-6.1-sol",
            "gpt-6.1-sol-2026-11-01"
        ));
        assert!(proxy_service::is_upstream_response_model_degraded(
            "gpt-6.2-sol",
            "gpt-6.1-sol"
        ));
        assert_eq!(
            tracker.record_observation(
                &make_obs(
                    "acc-mismatch",
                    "gpt-6.1-sol",
                    Some("gpt-6.1-sol-2026-11-01"),
                    Some("low"),
                    Some(0)
                ),
                200
            ),
            None
        );
        assert_eq!(
            tracker.record_observation(
                &make_obs(
                    "acc-mismatch",
                    "gpt-6.2-sol",
                    Some("gpt-4o"),
                    Some("low"),
                    Some(0)
                ),
                201
            ),
            Some("acc-mismatch".to_string())
        );
        assert!(tracker.is_temporarily_deprioritized("acc-mismatch", 205));

        // Periodic audit triggers after SENTINEL_AUDIT_HEALTHY_CALL_INTERVAL healthy heavy calls, without deprioritizing.
        for i in 0..(SENTINEL_AUDIT_HEALTHY_CALL_INTERVAL - 1) {
            assert_eq!(
                tracker.record_observation(
                    &make_obs("acc-audit", "gpt-6.2-sol", Some("gpt-6.2-sol"), Some("ultra"), Some(240)),
                    300 + i as i64
                ),
                None
            );
        }
        assert_eq!(
            tracker.record_observation(
                &make_obs("acc-audit", "gpt-6.2-sol", Some("gpt-6.2-sol"), Some("max"), Some(240)),
                310
            ),
            Some("acc-audit".to_string())
        );
    }

    #[test]
    fn sentinel_waiter_receives_probe_verdict_and_deprioritizes_suspect() {
        let mut tracker = SentinelTracker::default();
        let make_obs = |reasoning_tokens: i64| ProxySentinelObservation {
            account_key: "suspect@example.com|acc".to_string(),
            model: "gpt-6.2-sol".to_string(),
            response_model: Some("gpt-6.2-sol".to_string()),
            reasoning_effort: Some("xhigh".to_string()),
            input_tokens: Some(1500),
            output_tokens: Some(150),
            reasoning_tokens: Some(reasoning_tokens),
        };

        let (tx1, mut rx1) = oneshot::channel();
        assert_eq!(tracker.record_dispatch(&make_obs(0), Some(tx1), 100), None);
        assert_eq!(rx1.try_recv().unwrap(), false);
        assert!(!tracker.is_temporarily_deprioritized("suspect@example.com|acc", 100));

        let (tx2, mut rx2) = oneshot::channel();
        assert_eq!(tracker.record_dispatch(&make_obs(0), Some(tx2), 101), None);
        assert_eq!(rx2.try_recv().unwrap(), false);

        let (tx3, mut rx3) = oneshot::channel();
        assert_eq!(
            tracker.record_dispatch(&make_obs(0), Some(tx3), 102),
            Some("suspect@example.com|acc".to_string())
        );
        // Triggering call waits for probe completion; concurrent calls see temporary deprioritization.
        assert!(matches!(
            rx3.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(tracker.is_temporarily_deprioritized("suspect@example.com|acc", 105));

        let waiters = tracker.mark_probe_completed("suspect@example.com|acc", 110, true, true);
        for w in waiters {
            let _ = w.send(true);
        }
        assert_eq!(rx3.try_recv().unwrap(), true);
        assert!(!tracker.is_temporarily_deprioritized("suspect@example.com|acc", 110));
    }

    #[test]
    fn recovery_schedule_respects_backoff_quota_limits_and_window_resets() {
        let mut account = StoredAccount {
            id: "a1".to_string(),
            label: "acc".to_string(),
            source_kind: AccountSourceKind::Chatgpt,
            principal_id: Some("user@example.com".to_string()),
            email: Some("user@example.com".to_string()),
            account_id: "acc-1".to_string(),
            plan_type: Some("plus".to_string()),
            auth_json: json!({}),
            api_base_url: None,
            api_key: None,
            model_name: None,
            balance_text: None,
            profile_auth_path: None,
            profile_config_path: None,
            profile_auth_ready: true,
            profile_config_ready: true,
            profile_integrity_error: None,
            profile_last_validated_at: None,
            profile_last_validation_error: None,
            added_at: 100,
            updated_at: 100,
            usage: None,
            usage_error: None,
            auth_refresh_blocked: false,
            auth_refresh_error: None,
            api_proxy_enabled: false,
        };
        let mut summary = AccountFingerprintSummary {
            degraded: true,
            target_model: DEFAULT_PROBE_MODEL.to_string(),
            detected_model: "gpt-5.6-luna".to_string(),
            detected_model_id: "gpt-5.6-luna".to_string(),
            confidence: Some(0.99),
            score: Some(4.2),
            runner_up_model: None,
            runner_up_score: None,
            valid_samples: 2,
            total_samples: 2,
            ttft_ms: Some(1500),
            checked_at: 10_000,
            error: None,
            sol: None,
            astra: None,
            auto_disabled_by_fingerprint: true,
            auto_disabled_at: Some(10_000),
            recovery_attempts: 0,
            next_recovery_at: Some(compute_next_recovery_at(10_000, 0, None)),
        };

        assert_eq!(summary.next_recovery_at, Some(10_000 + 3600));
        assert_eq!(
            compute_next_recovery_at(10_000, 1, None),
            10_000 + 2 * 3600
        );
        assert_eq!(
            compute_next_recovery_at(10_000, 99, None),
            10_000 + 12 * 3600
        );
        assert_eq!(
            recovery_probe_due(&account, &summary, 11_000),
            RecoveryEligibility::Wait
        );
        assert_eq!(
            recovery_probe_due(&account, &summary, 13_600),
            RecoveryEligibility::Due
        );

        // Fresh 5h window reset unlocks an early recovery check once minimum interval has elapsed.
        account.usage = Some(UsageSnapshot {
            fetched_at: 11_600,
            plan_type: Some("plus".to_string()),
            five_hour: Some(UsageWindow {
                used_percent: 5.0,
                window_seconds: 18_000,
                reset_at: Some(11_500 + 18_000),
            }),
            one_week: None,
            credits: None,
            reset_credits: None,
        });
        assert_eq!(
            recovery_probe_due(&account, &summary, 11_600),
            RecoveryEligibility::Due
        );

        // High 5h quota (>=90%) defers probing until after reset_at + lead margin.
        account.usage.as_mut().unwrap().five_hour = Some(UsageWindow {
            used_percent: 92.0,
            window_seconds: 18_000,
            reset_at: Some(20_000),
        });
        assert_eq!(
            recovery_probe_due(&account, &summary, 15_000),
            RecoveryEligibility::DeferUntil(20_000 + RECOVERY_WINDOW_RESET_LEAD_SECS)
        );

        // Manual disable (auto_disabled_by_fingerprint=false) or auth failure must always skip.
        summary.auto_disabled_by_fingerprint = false;
        assert_eq!(
            recovery_probe_due(&account, &summary, 99_999),
            RecoveryEligibility::Skip
        );
        summary.auto_disabled_by_fingerprint = true;
        account.usage = None;
        account.usage_error = Some("401 Unauthorized: token_revoked".to_string());
        assert_eq!(
            recovery_probe_due(&account, &summary, 99_999),
            RecoveryEligibility::Skip
        );
    }
}
