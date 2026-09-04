#ifndef OMASHEETS_NATIVE_CLIPBOARD_H
#define OMASHEETS_NATIVE_CLIPBOARD_H
#include <QClipboard>
#include <QGuiApplication>
#include <QMimeData>
#include <QString>

// Called only by grid invokables on the GUI thread. Qt owns the MIME object.
inline void write_grid_clipboard(const QString &text, const QString &origin) {
    auto *mime = new QMimeData;
    mime->setText(text);
    mime->setData("application/x-omasheets-cells-v1", origin.toUtf8());
    QGuiApplication::clipboard()->setMimeData(mime);
}

inline QString grid_clipboard_origin(const QString &text) {
    const auto *mime = QGuiApplication::clipboard()->mimeData();
    if (!mime || mime->text() != text) return {};
    const auto data = mime->data("application/x-omasheets-cells-v1");
    if (data.size() > 256) return QStringLiteral("invalid");
    return QString::fromUtf8(data);
}
#endif
