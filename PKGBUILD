# Maintainer: John Barry
pkgname=flow
pkgver=0.3.2
pkgrel=1
pkgdesc="Voice dictation daemon with a local speech model and a system tray console"
arch=('x86_64')
url="https://github.com/Genoux/flow"
license=('MIT')
# gcc-libs/glibc omitted: makepkg's own shared-library scan already adds them
# from the linked binaries. wayland is kept explicit even though it isn't a
# hard ELF dependency (namcap: "may not be needed") — the wayland-client
# crate dlopens libwayland-client.so at runtime instead of linking it, which
# static NEEDED-entry scanning can't see, but the daemon genuinely requires
# it to reach the compositor.
depends=('alsa-lib' 'wayland' 'hicolor-icon-theme')
optdepends=('vulkan-icd-loader: GPU-accelerated local speech model inference')
install=flow.install
source=("https://github.com/Genoux/flow/releases/download/v${pkgver}/flow-v${pkgver}-${CARCH}-linux.tar.gz")
sha256sums=('e2f7271bd7199a09353784b78bbf83f6e65415cac0c1a26672a87d746f051a25')

package() {
	local srcname="flow-v${pkgver}-${CARCH}-linux"

	install -Dm755 "$srcdir/$srcname/bin/flow" "$pkgdir/usr/bin/flow"
	install -Dm755 "$srcdir/$srcname/bin/flow-console" "$pkgdir/usr/bin/flow-console"

	install -Dm644 "$srcdir/$srcname/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
	install -Dm644 "$srcdir/$srcname/README.md" "$pkgdir/usr/share/doc/$pkgname/README.md"
	install -Dm644 "$srcdir/$srcname/TROUBLESHOOTING.md" "$pkgdir/usr/share/doc/$pkgname/TROUBLESHOOTING.md"

	install -Dm644 "$srcdir/$srcname/packaging/flow-console.desktop" \
		"$pkgdir/usr/share/applications/flow-console.desktop"
	install -Dm644 "$srcdir/$srcname/packaging/flow-console.png" \
		"$pkgdir/usr/share/icons/hicolor/512x512/apps/flow-console.png"
	# The upstream user-level installer hardcodes an absolute icon path
	# because ~/.local/share/icons/hicolor has no index.theme there. A
	# system package installs into the real hicolor theme (which always has
	# one), so the bare theme name resolves correctly and themes with it.
	sed -i "s|^Icon=.*|Icon=flow-console|" "$pkgdir/usr/share/applications/flow-console.desktop"

	# User-level unit paths (%h/.local/bin/flow) rewritten to the pacman-
	# managed binary path this package actually installs to.
	install -Dm644 "$srcdir/$srcname/packaging/flow.service" \
		"$pkgdir/usr/lib/systemd/user/flow.service"
	install -Dm644 "$srcdir/$srcname/packaging/flow-tray.service" \
		"$pkgdir/usr/lib/systemd/user/flow-tray.service"
	sed -i "s|%h/.local/bin/flow|/usr/bin/flow|" \
		"$pkgdir/usr/lib/systemd/user/flow.service" \
		"$pkgdir/usr/lib/systemd/user/flow-tray.service"

	# Flow opens /dev/uinput to type. Shipped as a udev rule rather than the
	# printed sudo instructions the user-level installer gives, since a
	# pacman package can own a file under /etc that $HOME cannot reach.
	install -Dm644 /dev/stdin "$pkgdir/usr/lib/udev/rules.d/99-flow-uinput.rules" <<-'EOF'
	KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
	EOF
}
