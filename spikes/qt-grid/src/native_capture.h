#ifndef OMASHEETS_NATIVE_CAPTURE_H
#define OMASHEETS_NATIVE_CAPTURE_H
#include <QGuiApplication>
#include <QImage>
#include <QQuickWindow>
#include <QString>

// Screenshot evidence includes the palette, window content and popup overlay.
// Called on the GUI thread only when an explicit capture destination is set.
inline bool capture_grid_window(const QString &path) {
    if (path.isEmpty()) return false;
    for (auto *window : QGuiApplication::allWindows()) {
        if (window->objectName() != QStringLiteral("omasheetsWindow")) continue;
        auto *quick = qobject_cast<QQuickWindow *>(window);
        if (quick && quick->isVisible()) return quick->grabWindow().save(path);
    }
    return false;
}
#endif
