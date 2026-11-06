#include <QGuiApplication>
#include <QQmlContext>
#include <QQuickView>
#include <QQuickItem>
#include <QSGRendererInterface>
#include <QDebug>
#include <QTimer>
#include <cstdio>

int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    app.setDesktopFileName("qt-floor");
    bool ok = false;
    const int rows = app.arguments().value(1).toInt(&ok);
    if (!ok || (rows != 0 && rows != 100000)) return 2;
    // Disposable control palette, deliberately independent of user configuration.
    const QVariantMap tokens{{"background", "#181818"}, {"foreground", "#eeeeee"},
                             {"fontFamily", "DejaVu Sans"}, {"fontSize", 14},
                             {"rowHeight", 28}};
    QQuickView view;
    view.setTitle("Qt floor");
    view.setResizeMode(QQuickView::SizeRootObjectToView);
    view.rootContext()->setContextProperty("floorTokens", tokens);
    view.rootContext()->setContextProperty("floorRows", rows);
    view.setSource(QUrl("qrc:/Floor.qml"));
    if (view.status() != QQuickView::Ready) return 3;
    view.resize(1280, 900);
    QObject::connect(&view, &QQuickWindow::sceneGraphInitialized, &view, [&view] {
        qInfo() << "FLOOR_RENDERER" << view.rendererInterface()->graphicsApi()
                << "Qt" << qVersion() << "size" << view.size();
    }, Qt::DirectConnection);
    view.show();
    QTimer::singleShot(2500, &view, [&view, rows] {
        // A stable console record also works on Qt builds defaulting to journald.
        std::fprintf(stderr, "FLOOR_SETTLED rows=%d qt=%s api=%d size=%dx%d\n", rows,
                     qVersion(), int(view.rendererInterface()->graphicsApi()), view.width(), view.height());
        std::fflush(stderr);
    });
    return app.exec();
}
