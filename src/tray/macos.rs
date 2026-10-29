//! The macOS menu bar item: an `NSStatusItem` with an `NSMenu`, and launchd for
//! the actions.
//!
//! AppKit is for the main thread, and the loop that decides whether the icon
//! shows is not on it, so showing and hiding are handed to the main queue like
//! the overlay's surface. The menu is rebuilt each time it opens, because
//! whether dictation is running is a question for launchd and not something to
//! remember.

use super::{Action, Entry, Verb};
use dispatch2::DispatchQueue;
use flow_paths::launchd::{Agent, Runner, Verb as LaunchVerb};
use flow_paths::process::{BUDGET, run_for};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSSize, NSString};
use std::cell::RefCell;
use std::process::{Command, Output};
use std::sync::{Mutex, PoisonError};

/// The one monochrome mark the Linux tray also uses. Drawn as a template, so
/// only its alpha counts and the menu bar tints it for light and dark.
const ICON: &[u8] = include_bytes!("../../assets/tray.png");

/// Standard menu bar glyph height, in points.
const ICON_POINTS: f64 = 18.0;

const DICTATION: Agent = Agent {
    label: "com.genoux.flow",
};
const TRAY: Agent = Agent {
    label: "com.genoux.flow.tray",
};

static ACTIONS: Mutex<()> = Mutex::new(());

const CHOICES: [Action; 4] = [
    Action::Open,
    Action::ToggleDictation,
    Action::RestartDictation,
    Action::Quit,
];

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlowTrayTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild(menu);
        }
    }

    impl Target {
        #[unsafe(method(choose:))]
        fn choose(&self, sender: &NSMenuItem) {
            if let Some(action) = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| CHOICES.get(tag))
                .copied()
            {
                // Off the main thread: launchctl can take a while, and a menu
                // that freezes the menu bar while it waits is worse than one
                // that answers late.
                std::thread::spawn(move || {
                    // One at a time: two quick clicks would otherwise run two
                    // launchctl sequences over each other.
                    let _one_at_a_time = ACTIONS.lock().unwrap_or_else(PoisonError::into_inner);
                    super::perform(action);
                });
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }

    fn rebuild(&self, menu: &NSMenu) {
        menu.removeAllItems();
        for entry in super::menu(dictation_running()) {
            let item = match entry {
                Entry::Separator => NSMenuItem::separatorItem(self.mtm()),
                Entry::Item { label, action } => {
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            self.mtm().alloc(),
                            &NSString::from_str(label),
                            Some(sel!(choose:)),
                            &NSString::new(),
                        )
                    };
                    let tag = CHOICES.iter().position(|known| *known == action);
                    item.setTag(tag.map_or(-1, |tag| tag as isize));
                    unsafe { item.setTarget(Some(self)) };
                    item
                }
            };
            menu.addItem(&item);
        }
    }
}

/// What the main thread holds while the icon is up.
struct Shown {
    item: Retained<NSStatusItem>,
    _target: Retained<Target>,
}

thread_local! {
    /// Main thread only, like everything that touches AppKit.
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

pub struct Icon;

impl Icon {
    pub fn hide(self) {
        DispatchQueue::main().exec_async(|| {
            SHOWN.with_borrow_mut(|shown| {
                if let Some(Shown { item, .. }) = shown.take() {
                    NSStatusBar::systemStatusBar().removeStatusItem(&item);
                    eprintln!("tray icon hidden");
                }
            });
        });
    }
}

pub fn show() -> Option<Icon> {
    DispatchQueue::main().exec_async(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        SHOWN.with_borrow_mut(|shown| {
            if shown.is_none() {
                *shown = Some(make_item(mtm));
                eprintln!("tray icon shown");
            }
        });
    });
    Some(Icon)
}

fn make_item(mtm: MainThreadMarker) -> Shown {
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let target = Target::new(mtm);

    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    item.setMenu(Some(&menu));

    if let Some(button) = item.button(mtm) {
        let image = NSImage::initWithData(mtm.alloc(), &NSData::with_bytes(ICON));
        if let Some(image) = image {
            image.setSize(NSSize::new(ICON_POINTS, ICON_POINTS));
            image.setTemplate(true);
            button.setImage(Some(&image));
        }
    }
    Shown {
        item,
        _target: target,
    }
}

/// With the console's own deadline: a `launchctl` that hangs must not hold the
/// one-at-a-time lock for good, which would leave every later menu click dead.
fn launchctl(args: &[&str]) -> Option<Output> {
    run_for("launchctl", args, BUDGET)
}

fn report(what: &str, outcome: Result<(), String>) {
    if let Err(err) = outcome {
        eprintln!("tray: {what}: {err}");
    }
}

/// Opens Flow.app, which is the console. Opening an app that is already
/// running brings its window forward rather than making a second one.
pub fn open_console() {
    let Some(bundle) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.ancestors().nth(3).map(std::path::Path::to_path_buf))
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
    else {
        eprintln!("tray: not running from an app bundle, cannot open Flow");
        return;
    };
    if let Err(err) = Command::new("open").arg(bundle).status() {
        eprintln!("tray: could not open Flow: {err}");
    }
}

/// Whether the daemon is answering on its status socket.
///
/// Asked this way because the menu is built on the main thread, which must
/// never wait on a `launchctl` child with no timeout. Connecting to a local
/// socket either succeeds or is refused at once, and a socket left behind by a
/// crashed daemon is refused, so it reads as not running.
pub fn dictation_running() -> bool {
    std::os::unix::net::UnixStream::connect(flow_paths::socket()).is_ok()
}

pub fn dictation(verb: Verb) {
    let verb = match verb {
        Verb::Start => LaunchVerb::Start,
        Verb::Stop => LaunchVerb::Stop,
        Verb::Restart => LaunchVerb::Restart,
    };
    report("dictation", DICTATION.control(verb, &launchctl as Runner));
}

/// End Flow entirely: the daemon, then this controller. Unloading our own job
/// is what makes this Quit rather than a second way to hide the icon -
/// `show_tray = false` leaves the controller running so the icon can come back
/// live. launchd ends this process as part of the call, so nothing after it is
/// reached.
pub fn quit() {
    // Stays up if the daemon would not stop: the menu is then still the user's
    // way to try again, and a tray that quit anyway would leave a running
    // daemon with nothing to control it.
    if let Err(err) = DICTATION.control(LaunchVerb::Stop, &launchctl as Runner) {
        eprintln!("tray: quit: {err}");
        return;
    }
    // Launchd saying the daemon is stopped is not the same as it being gone: one
    // started by hand, outside launchd, is not its to stop. The menu reads the
    // socket, so Quit goes by the socket too, and stays rather than leave a
    // running daemon with no controller.
    if dictation_running() {
        eprintln!("tray: quit: dictation is still running and was not started by launchd");
        return;
    }
    report("quit", TRAY.control(LaunchVerb::Stop, &launchctl as Runner));
}
