// Invoked only by the Rust Qt integration test, never by the application UI.
#include "gust_model_probe.h"
#include "gust-app/src/bridge.cxxqt.h"
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QPersistentModelIndex>
#include <QtCore/QVariant>

namespace {
class Probe final : public QObject {
public:
    explicit Probe(DirModel &model) : QObject(&model), model(model) {
        setObjectName(QStringLiteral("gust-projection-test-probe"));
        QObject::connect(&model, &QAbstractItemModel::modelAboutToBeReset, this, [this] { begin(false); });
        QObject::connect(&model, &QAbstractItemModel::modelReset, this, [this] { end(false); });
        QObject::connect(&model, &QAbstractItemModel::layoutAboutToBeChanged, this,
            [this](const QList<QPersistentModelIndex> &, QAbstractItemModel::LayoutChangeHint hint) {
                if (hint != QAbstractItemModel::VerticalSortHint) ok = false;
                begin(true);
            });
        QObject::connect(&model, &QAbstractItemModel::layoutChanged, this,
            [this](const QList<QPersistentModelIndex> &, QAbstractItemModel::LayoutChangeHint hint) {
                if (hint != QAbstractItemModel::VerticalSortHint) ok = false;
                end(true);
            });
        QObject::connect(&model, &DirModel::locationChanged, this, [this] { ++locations; });
        QObject::connect(&model, &DirModel::cursorChanged, this, [this] { if (open) ok = false; ++cursors; });
        QObject::connect(&model, &DirModel::rows_replaced, this, [this](int keep) {
            if (open) ok = false;
            if (keep < 0 || keep > 2) ok = false;
            for (int row = 0; row < oldNames.size(); ++row) {
                const int mapped = this->model.published_row(row);
                if (mapped >= 0 && (mapped >= this->model.rowCount({}) || this->model.entry_name(mapped) != oldNames[row])) ok = false;
            }
            ++publications;
        });
    }
    void begin(bool layout) {
        if (open || model.rowCount({}) != model.getCount()) ok = false;
        open = true;
        oldNames.clear();
        for (int row = 0; row < model.rowCount({}); ++row) oldNames.append(model.entry_name(row));
        if (layout) ++layouts; else ++resets;
    }
    void end(bool) {
        if (!open || model.rowCount({}) != model.getCount()) ok = false;
        open = false;
        if (remembered.isValid() && model.entry_name(remembered.row()) != rememberedName) ok = false;
        if (nameReads + displayReads + otherReads > 0 && model.rowCount({}) > 0) {
            // Reenter Rust diagnostics both before and after completed data()
            // bodies while the synchronous end_* signal is still on the stack.
            const auto before = callbacks();
            const auto index = model.index(0, 0, {});
            for (int i = 0; i < nameReads; ++i) model.data(index, Qt::UserRole);
            for (int i = 0; i < displayReads; ++i) model.data(index, Qt::DisplayRole);
            for (int i = 0; i < otherReads; ++i) model.data(index, Qt::UserRole + 1);
            const auto after = callbacks();
            const int calls = after["data"].toObject()["count"].toInt() - before["data"].toObject()["count"].toInt();
            if (calls != nameReads + displayReads + otherReads) ok = false;
            const auto events = after["publication_events"].toObject();
            reentrantEvents = events["resets"].toObject()["count"].toInt() + events["layouts"].toObject()["count"].toInt();
            reentrantEnabled = events["enabled"].toBool();
            ++readRounds;
            readCalls += calls;
        }
    }
    QJsonObject callbacks() const {
        return QJsonDocument::fromJson(model.state_json().toUtf8()).object()["projection"].toObject()["callbacks"].toObject();
    }
    DirModel &model;
    bool open = false, ok = true;
    int layouts = 0, resets = 0, publications = 0, locations = 0, cursors = 0;
    int nameReads = 0, displayReads = 0, otherReads = 0, readRounds = 0, readCalls = 0, reentrantEvents = 0;
    bool reentrantEnabled = false;
    QStringList oldNames;
    QPersistentModelIndex remembered;
    QString rememberedName;
};
Probe *probe(const DirModel &model) {
    for (QObject *child : model.children()) {
        if (child->objectName() == QStringLiteral("gust-projection-test-probe")) return static_cast<Probe *>(child);
    }
    return nullptr;
}
}
std::unique_ptr<DirModel> gust_test_new_model() { return std::make_unique<DirModel>(); }
void gust_test_attach_probe(DirModel &model) { new Probe(model); }
void gust_test_publication_reads(DirModel &model, std::int32_t name, std::int32_t display, std::int32_t other) {
    auto *p = probe(model);
    p->nameReads = name;
    p->displayReads = display;
    p->otherReads = other;
}
void gust_test_remember_index(DirModel &model, std::int32_t row) {
    auto *p = probe(model);
    p->remembered = model.index(row, 0, {});
    p->rememberedName = model.entry_name(row);
}
QString gust_test_probe_state(const DirModel &model) {
    const auto *p = probe(model);
    return QString::fromUtf8(QJsonDocument(QJsonObject{{"ok",p->ok},{"open",p->open},{"layouts",p->layouts},
        {"resets",p->resets},{"publications",p->publications},{"locations",p->locations},{"cursors",p->cursors},{"persistent_row",p->remembered.row()},
        {"persistent_name",p->rememberedName},{"read_rounds",p->readRounds},{"read_calls",p->readCalls},
        {"reentrant_events",p->reentrantEvents},{"reentrant_enabled",p->reentrantEnabled}}).toJson(QJsonDocument::Compact));
}
