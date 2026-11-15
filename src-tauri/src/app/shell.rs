use std::sync::Mutex;

use crate::app::op_phase::QuitPolicy;

pub const PRODUCT_NAME: &str = "Codex App Manager";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLocale {
    En,
    ZhCn,
    ZhTw,
    Ja,
    Ko,
    Fr,
    De,
    Es,
    PtBr,
    Ru,
    Ar,
}

impl NativeLocale {
    pub fn from_tag(tag: &str) -> Self {
        match tag.trim().to_ascii_lowercase().as_str() {
            "zh-cn" | "zh-hans" => Self::ZhCn,
            "zh-tw" | "zh-hant" => Self::ZhTw,
            "ja" => Self::Ja,
            "ko" => Self::Ko,
            "fr" => Self::Fr,
            "de" => Self::De,
            "es" => Self::Es,
            "pt-br" => Self::PtBr,
            "ru" => Self::Ru,
            "ar" => Self::Ar,
            _ => Self::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
            Self::Ja => "ja",
            Self::Ko => "ko",
            Self::Fr => "fr",
            Self::De => "de",
            Self::Es => "es",
            Self::PtBr => "pt-BR",
            Self::Ru => "ru",
            Self::Ar => "ar",
        }
    }

    pub fn menu(self) -> NativeMenuCopy {
        match self {
            Self::En => NativeMenuCopy {
                about: "About Codex App Manager",
                services: "Services",
                hide: "Hide Codex App Manager",
                hide_others: "Hide Others",
                show_all: "Show All",
                quit: "Quit Codex App Manager",
                edit: "Edit",
                undo: "Undo",
                redo: "Redo",
                cut: "Cut",
                copy: "Copy",
                paste: "Paste",
                select_all: "Select All",
                window: "Window",
                minimize: "Minimize",
                close_window: "Close Window",
                tray_show: "Show Codex App Manager",
                tray_quit: "Quit",
            },
            Self::ZhCn => NativeMenuCopy {
                about: "关于 Codex App Manager",
                services: "服务",
                hide: "隐藏 Codex App Manager",
                hide_others: "隐藏其他",
                show_all: "全部显示",
                quit: "退出 Codex App Manager",
                edit: "编辑",
                undo: "撤销",
                redo: "重做",
                cut: "剪切",
                copy: "拷贝",
                paste: "粘贴",
                select_all: "全选",
                window: "窗口",
                minimize: "最小化",
                close_window: "关闭窗口",
                tray_show: "显示 Codex App Manager",
                tray_quit: "退出",
            },
            Self::ZhTw => NativeMenuCopy {
                about: "關於 Codex App Manager",
                services: "服務",
                hide: "隱藏 Codex App Manager",
                hide_others: "隱藏其他",
                show_all: "全部顯示",
                quit: "結束 Codex App Manager",
                edit: "編輯",
                undo: "還原",
                redo: "重做",
                cut: "剪下",
                copy: "複製",
                paste: "貼上",
                select_all: "全選",
                window: "視窗",
                minimize: "最小化",
                close_window: "關閉視窗",
                tray_show: "顯示 Codex App Manager",
                tray_quit: "結束",
            },
            Self::Ja => NativeMenuCopy {
                about: "Codex App Managerについて",
                services: "サービス",
                hide: "Codex App Managerを隠す",
                hide_others: "ほかを隠す",
                show_all: "すべてを表示",
                quit: "Codex App Managerを終了",
                edit: "編集",
                undo: "取り消す",
                redo: "やり直す",
                cut: "カット",
                copy: "コピー",
                paste: "ペースト",
                select_all: "すべてを選択",
                window: "ウインドウ",
                minimize: "しまう",
                close_window: "ウインドウを閉じる",
                tray_show: "Codex App Managerを表示",
                tray_quit: "Codex App Managerを終了",
            },
            Self::Ko => NativeMenuCopy {
                about: "Codex App Manager에 관하여",
                services: "서비스",
                hide: "Codex App Manager 가리기",
                hide_others: "기타 가리기",
                show_all: "모두 보기",
                quit: "Codex App Manager 종료",
                edit: "편집",
                undo: "실행 취소",
                redo: "실행 복귀",
                cut: "오려두기",
                copy: "복사",
                paste: "붙여넣기",
                select_all: "모두 선택",
                window: "윈도우",
                minimize: "최소화",
                close_window: "윈도우 닫기",
                tray_show: "Codex App Manager 보기",
                tray_quit: "Codex App Manager 종료",
            },
            Self::Fr => NativeMenuCopy {
                about: "À propos de Codex App Manager",
                services: "Services",
                hide: "Masquer Codex App Manager",
                hide_others: "Masquer les autres",
                show_all: "Tout afficher",
                quit: "Quitter Codex App Manager",
                edit: "Édition",
                undo: "Annuler",
                redo: "Rétablir",
                cut: "Couper",
                copy: "Copier",
                paste: "Coller",
                select_all: "Tout sélectionner",
                window: "Fenêtre",
                minimize: "Placer dans le Dock",
                close_window: "Fermer la fenêtre",
                tray_show: "Afficher Codex App Manager",
                tray_quit: "Quitter",
            },
            Self::De => NativeMenuCopy {
                about: "Über Codex App Manager",
                services: "Dienste",
                hide: "Codex App Manager ausblenden",
                hide_others: "Andere ausblenden",
                show_all: "Alle einblenden",
                quit: "Codex App Manager beenden",
                edit: "Bearbeiten",
                undo: "Widerrufen",
                redo: "Wiederholen",
                cut: "Ausschneiden",
                copy: "Kopieren",
                paste: "Einsetzen",
                select_all: "Alles auswählen",
                window: "Fenster",
                minimize: "Im Dock ablegen",
                close_window: "Fenster schließen",
                tray_show: "Codex App Manager anzeigen",
                tray_quit: "Beenden",
            },
            Self::Es => NativeMenuCopy {
                about: "Acerca de Codex App Manager",
                services: "Servicios",
                hide: "Ocultar Codex App Manager",
                hide_others: "Ocultar otros",
                show_all: "Mostrar todo",
                quit: "Salir de Codex App Manager",
                edit: "Edición",
                undo: "Deshacer",
                redo: "Rehacer",
                cut: "Cortar",
                copy: "Copiar",
                paste: "Pegar",
                select_all: "Seleccionar todo",
                window: "Ventana",
                minimize: "Minimizar",
                close_window: "Cerrar ventana",
                tray_show: "Mostrar Codex App Manager",
                tray_quit: "Salir",
            },
            Self::PtBr => NativeMenuCopy {
                about: "Sobre o Codex App Manager",
                services: "Serviços",
                hide: "Ocultar Codex App Manager",
                hide_others: "Ocultar Outros",
                show_all: "Mostrar Tudo",
                quit: "Encerrar Codex App Manager",
                edit: "Editar",
                undo: "Desfazer",
                redo: "Refazer",
                cut: "Cortar",
                copy: "Copiar",
                paste: "Colar",
                select_all: "Selecionar Tudo",
                window: "Janela",
                minimize: "Minimizar",
                close_window: "Fechar Janela",
                tray_show: "Mostrar Codex App Manager",
                tray_quit: "Encerrar",
            },
            Self::Ru => NativeMenuCopy {
                about: "О программе Codex App Manager",
                services: "Службы",
                hide: "Скрыть Codex App Manager",
                hide_others: "Скрыть остальные",
                show_all: "Показать все",
                quit: "Завершить Codex App Manager",
                edit: "Правка",
                undo: "Отменить",
                redo: "Повторить",
                cut: "Вырезать",
                copy: "Копировать",
                paste: "Вставить",
                select_all: "Выбрать все",
                window: "Окно",
                minimize: "Свернуть",
                close_window: "Закрыть окно",
                tray_show: "Показать Codex App Manager",
                tray_quit: "Завершить",
            },
            Self::Ar => NativeMenuCopy {
                about: "حول Codex App Manager",
                services: "الخدمات",
                hide: "إخفاء Codex App Manager",
                hide_others: "إخفاء الآخرين",
                show_all: "إظهار الكل",
                quit: "إنهاء Codex App Manager",
                edit: "تحرير",
                undo: "تراجع",
                redo: "إعادة",
                cut: "قص",
                copy: "نسخ",
                paste: "لصق",
                select_all: "تحديد الكل",
                window: "نافذة",
                minimize: "تصغير",
                close_window: "إغلاق النافذة",
                tray_show: "إظهار Codex App Manager",
                tray_quit: "إنهاء",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeMenuCopy {
    pub about: &'static str,
    pub services: &'static str,
    pub hide: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub quit: &'static str,
    pub edit: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub cut: &'static str,
    pub copy: &'static str,
    pub paste: &'static str,
    pub select_all: &'static str,
    pub window: &'static str,
    pub minimize: &'static str,
    pub close_window: &'static str,
    pub tray_show: &'static str,
    pub tray_quit: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    ConfirmQuit,
    QuitBlocked(QuitPolicy),
}

impl ShellEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ConfirmQuit => "confirm-quit",
            Self::QuitBlocked(_) => "quit-blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellDispatch {
    Emit(ShellEvent),
    Native(ShellEvent),
    Queued { pending: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontendLoad {
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendToken {
    pub generation: u64,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendReady {
    pub generation: u64,
    pub first_ready: bool,
    pub degraded: bool,
    pub activation_pending: bool,
    pub pending: Vec<ShellEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendReadyResult {
    Accepted(FrontendReady),
    Stale { current_generation: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendDegraded {
    pub activation_pending: bool,
    pub next_native_event: Option<ShellEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendFailureResult {
    Accepted(FrontendDegraded),
    Stale { current_generation: u64 },
}

#[derive(Debug, Default)]
struct FrontendGateInner {
    ready: bool,
    degraded: bool,
    generation: u64,
    token: Option<String>,
    activation_pending: bool,
    native_dialog_active: bool,
    pending: Vec<ShellEvent>,
}

#[derive(Debug, Default)]
pub struct FrontendGate {
    inner: Mutex<FrontendGateInner>,
}

impl FrontendGate {
    fn queue_event(inner: &mut FrontendGateInner, event: ShellEvent) {
        // Startup can receive repeated Cmd+Q/close requests before React has
        // registered both listeners. Keep only the latest event of each kind:
        // the queue remains bounded while no user intent is silently lost.
        if let Some(index) = inner
            .pending
            .iter()
            .position(|queued| queued.kind() == event.kind())
        {
            inner.pending.remove(index);
        }
        // Re-append replacements so different event kinds retain the order of
        // their latest occurrence, not the order in which each kind first
        // appeared.
        inner.pending.push(event);
    }

    fn take_next_native(inner: &mut FrontendGateInner) -> Option<ShellEvent> {
        if !inner.degraded || inner.native_dialog_active || inner.pending.is_empty() {
            return None;
        }
        inner.native_dialog_active = true;
        Some(inner.pending.remove(0))
    }

    fn enter_degraded(inner: &mut FrontendGateInner) -> FrontendDegraded {
        inner.ready = false;
        inner.degraded = true;
        let activation_pending = std::mem::take(&mut inner.activation_pending);
        let next_native_event = Self::take_next_native(inner);
        FrontendDegraded {
            activation_pending,
            next_native_event,
        }
    }

    pub fn route(&self, event: ShellEvent) -> ShellDispatch {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.ready {
            return ShellDispatch::Emit(event);
        }

        Self::queue_event(&mut inner, event);
        if let Some(event) = Self::take_next_native(&mut inner) {
            ShellDispatch::Native(event)
        } else {
            ShellDispatch::Queued {
                pending: inner.pending.len(),
            }
        }
    }

    pub fn mark_loading(&self) -> FrontendLoad {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.generation = inner.generation.wrapping_add(1);
        inner.ready = false;
        inner.degraded = false;
        inner.token = Some(uuid::Uuid::new_v4().to_string());
        FrontendLoad {
            generation: inner.generation,
        }
    }

    pub fn current_token(&self) -> Option<FrontendToken> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.token.as_ref().map(|token| FrontendToken {
            generation: inner.generation,
            token: token.clone(),
        })
    }

    pub fn request_activation(&self) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.ready || inner.degraded {
            true
        } else {
            inner.activation_pending = true;
            false
        }
    }

    /// Whether the current renderer generation can safely own window
    /// presentation. A timed-out generation is also presentable because native
    /// degraded mode must be able to surface its fallback dialogs.
    pub fn can_present_window(&self) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.ready || inner.degraded
    }

    pub fn mark_ready(&self, generation: u64, token: &str) -> FrontendReadyResult {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.generation != generation || inner.token.as_deref() != Some(token) {
            return FrontendReadyResult::Stale {
                current_generation: inner.generation,
            };
        }
        // Once this document has timed out, keep native delivery latched until
        // the next PageLoad Started creates a fresh generation. A late WebView
        // handshake may update locale/title, but must not steal queued quit
        // decisions back from the native fallback while a dialog is active.
        if inner.degraded {
            return FrontendReadyResult::Accepted(FrontendReady {
                generation: inner.generation,
                first_ready: false,
                degraded: true,
                activation_pending: false,
                pending: Vec::new(),
            });
        }
        let first_ready = !inner.ready;
        inner.ready = true;
        let activation_pending = std::mem::take(&mut inner.activation_pending);
        FrontendReadyResult::Accepted(FrontendReady {
            generation: inner.generation,
            first_ready,
            degraded: false,
            activation_pending,
            pending: std::mem::take(&mut inner.pending),
        })
    }

    pub fn mark_degraded(&self, generation: u64) -> Option<FrontendDegraded> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.generation != generation || inner.ready || inner.degraded {
            return None;
        }
        Some(Self::enter_degraded(&mut inner))
    }

    /// A renderer that previously completed the ready handshake can still lose
    /// its quit listeners when the root React boundary replaces the app tree.
    /// Authenticate that exact document before latching native delivery; a
    /// delayed failure report from an older document must not degrade a reload.
    pub fn mark_failed(&self, generation: u64, token: &str) -> FrontendFailureResult {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.generation != generation || inner.token.as_deref() != Some(token) {
            return FrontendFailureResult::Stale {
                current_generation: inner.generation,
            };
        }
        if inner.degraded {
            return FrontendFailureResult::Accepted(FrontendDegraded {
                activation_pending: false,
                next_native_event: None,
            });
        }
        FrontendFailureResult::Accepted(Self::enter_degraded(&mut inner))
    }

    pub fn native_dialog_finished(&self) -> Option<ShellEvent> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.native_dialog_active = false;
        Self::take_next_native(&mut inner)
    }

    pub fn is_waiting_for(&self, generation: u64) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.generation == generation && !inner.ready && !inner.degraded
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FrontendFailureResult, FrontendGate, FrontendReadyResult, NativeLocale, ShellDispatch,
        ShellEvent,
    };
    use crate::app::op_phase::{OperationPhase, QuitPolicy};

    fn blocked(reason: &str) -> ShellEvent {
        ShellEvent::QuitBlocked(QuitPolicy::Block {
            phase: OperationPhase::Committing,
            reason_code: "committing".to_string(),
            reason: reason.to_string(),
            kind: Some("update".to_string()),
        })
    }

    fn start_load(gate: &FrontendGate) -> (u64, String) {
        let load = gate.mark_loading();
        let token = gate.current_token().expect("load token");
        assert_eq!(token.generation, load.generation);
        (load.generation, token.token)
    }

    #[test]
    fn native_locale_maps_all_supported_tags_and_falls_back_to_english() {
        let cases = [
            ("en", NativeLocale::En),
            ("zh-CN", NativeLocale::ZhCn),
            ("zh-TW", NativeLocale::ZhTw),
            ("ja", NativeLocale::Ja),
            ("ko", NativeLocale::Ko),
            ("fr", NativeLocale::Fr),
            ("de", NativeLocale::De),
            ("es", NativeLocale::Es),
            ("pt-BR", NativeLocale::PtBr),
            ("ru", NativeLocale::Ru),
            ("ar", NativeLocale::Ar),
        ];
        for (tag, locale) in cases {
            assert_eq!(NativeLocale::from_tag(tag), locale, "{tag}");
            let menu = locale.menu();
            assert!(!menu.edit.is_empty(), "{tag}:edit");
            assert!(!menu.window.is_empty(), "{tag}:window");
            assert!(menu.quit.contains("Codex App Manager"), "{tag}:quit");
        }
        assert_eq!(NativeLocale::from_tag("unsupported"), NativeLocale::En);
    }

    #[test]
    fn startup_events_are_coalesced_then_drained_atomically() {
        let gate = FrontendGate::default();
        let (_, token) = start_load(&gate);
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Queued { pending: 1 }
        );
        assert_eq!(
            gate.route(blocked("old")),
            ShellDispatch::Queued { pending: 2 }
        );
        assert_eq!(
            gate.route(blocked("latest")),
            ShellDispatch::Queued { pending: 2 }
        );

        let FrontendReadyResult::Accepted(ready) = gate.mark_ready(1, &token) else {
            panic!("current token must be accepted");
        };
        assert!(ready.first_ready);
        assert!(!ready.degraded);
        assert!(!ready.activation_pending);
        assert_eq!(ready.pending.len(), 2);
        assert!(matches!(ready.pending[0], ShellEvent::ConfirmQuit));
        assert!(matches!(
            &ready.pending[1],
            ShellEvent::QuitBlocked(QuitPolicy::Block { reason, .. }) if reason == "latest"
        ));
        assert!(!gate.is_waiting_for(1));

        let gate = FrontendGate::default();
        let (_, token) = start_load(&gate);
        gate.route(blocked("old"));
        gate.route(ShellEvent::ConfirmQuit);
        gate.route(blocked("latest"));
        let FrontendReadyResult::Accepted(ready) = gate.mark_ready(1, &token) else {
            panic!("current token must be accepted");
        };
        assert!(matches!(ready.pending[0], ShellEvent::ConfirmQuit));
        assert!(matches!(
            &ready.pending[1],
            ShellEvent::QuitBlocked(QuitPolicy::Block { reason, .. }) if reason == "latest"
        ));
    }

    #[test]
    fn events_emit_immediately_after_frontend_is_ready() {
        let gate = FrontendGate::default();
        assert!(!gate.can_present_window());
        let (_, token) = start_load(&gate);
        assert!(matches!(
            gate.mark_ready(1, &token),
            FrontendReadyResult::Accepted(ref ready) if ready.first_ready && ready.pending.is_empty()
        ));
        assert!(gate.can_present_window());
        assert!(matches!(
            gate.mark_ready(1, &token),
            FrontendReadyResult::Accepted(ref ready) if !ready.first_ready && ready.pending.is_empty()
        ));
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Emit(ShellEvent::ConfirmQuit)
        );
    }

    #[test]
    fn authenticated_root_failure_switches_ready_delivery_to_native_fallback() {
        let gate = FrontendGate::default();
        let (generation, token) = start_load(&gate);
        assert!(matches!(
            gate.mark_ready(generation, &token),
            FrontendReadyResult::Accepted(ref ready) if ready.first_ready
        ));
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Emit(ShellEvent::ConfirmQuit)
        );

        assert_eq!(
            gate.mark_failed(generation, "wrong-token"),
            FrontendFailureResult::Stale {
                current_generation: generation
            }
        );
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Emit(ShellEvent::ConfirmQuit)
        );

        assert!(matches!(
            gate.mark_failed(generation, &token),
            FrontendFailureResult::Accepted(ref degraded)
                if !degraded.activation_pending && degraded.next_native_event.is_none()
        ));
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Native(ShellEvent::ConfirmQuit)
        );
        assert!(gate.can_present_window());
        assert!(matches!(
            gate.mark_ready(generation, &token),
            FrontendReadyResult::Accepted(ref ready) if ready.degraded && !ready.first_ready
        ));
    }

    #[test]
    fn stale_document_token_cannot_ready_a_new_renderer_generation() {
        let gate = FrontendGate::default();
        let (_, stale_token) = start_load(&gate);
        let (generation, current_token) = start_load(&gate);
        assert!(gate.is_waiting_for(generation));
        assert!(!gate.request_activation());

        assert_eq!(
            gate.mark_ready(generation - 1, &stale_token),
            FrontendReadyResult::Stale {
                current_generation: generation
            }
        );
        assert!(gate.is_waiting_for(generation));
        assert_eq!(
            gate.mark_ready(generation, "wrong-token"),
            FrontendReadyResult::Stale {
                current_generation: generation
            }
        );
        assert!(gate.is_waiting_for(generation));

        let FrontendReadyResult::Accepted(ready) = gate.mark_ready(generation, &current_token)
        else {
            panic!("current token must be accepted");
        };
        assert!(ready.first_ready);
        assert!(ready.activation_pending);
        assert!(ready.pending.is_empty());
        assert!(!gate.is_waiting_for(generation));
        assert!(gate.request_activation());
    }

    #[test]
    fn timeout_switches_to_serial_native_delivery_without_dropping_events() {
        let gate = FrontendGate::default();
        let (generation, token) = start_load(&gate);
        gate.route(ShellEvent::ConfirmQuit);
        assert!(!gate.request_activation());

        let degraded = gate
            .mark_degraded(generation)
            .expect("current load degrades");
        assert!(gate.can_present_window());
        assert!(degraded.activation_pending);
        assert!(matches!(
            degraded.next_native_event,
            Some(ShellEvent::ConfirmQuit)
        ));
        assert!(!gate.is_waiting_for(generation));
        assert!(gate.request_activation());

        assert_eq!(
            gate.route(blocked("critical")),
            ShellDispatch::Queued { pending: 1 }
        );
        let FrontendReadyResult::Accepted(late_ready) = gate.mark_ready(generation, &token) else {
            panic!("the current document token is still authentic");
        };
        assert!(late_ready.degraded);
        assert!(!late_ready.first_ready);
        assert!(late_ready.pending.is_empty());
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Queued { pending: 2 }
        );
        assert!(matches!(
            gate.native_dialog_finished(),
            Some(ShellEvent::QuitBlocked(QuitPolicy::Block { reason, .. })) if reason == "critical"
        ));
        assert!(matches!(
            gate.native_dialog_finished(),
            Some(ShellEvent::ConfirmQuit)
        ));
        assert_eq!(gate.native_dialog_finished(), None);
        assert_eq!(
            gate.route(ShellEvent::ConfirmQuit),
            ShellDispatch::Native(ShellEvent::ConfirmQuit)
        );
    }
}
