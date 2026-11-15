#pragma once
#include <QtCore/QAbstractListModel>
#include <rust/cxx.h>
#include <cstdint>

// The concrete Rust model mutates its immutable frame between these calls.
// Qt persistent indices track exact old-to-new rows prepared on the worker.
class GustListModel : public QAbstractListModel {
public:
    using QAbstractListModel::QAbstractListModel;
    void beginSortLayout() {
        emit layoutAboutToBeChanged({}, QAbstractItemModel::VerticalSortHint);
        oldPersistent = persistentIndexList();
    }
    void endSortLayout(rust::Slice<const std::int32_t> rows) {
        QModelIndexList mapped;
        mapped.reserve(oldPersistent.size());
        for (const QModelIndex &old : oldPersistent) {
            const auto row = old.row();
            const auto next = row >= 0 && std::size_t(row) < rows.size() ? rows[std::size_t(row)] : -1;
            mapped.append(next < 0 ? QModelIndex{} : index(next, old.column(), old.parent()));
        }
        changePersistentIndexList(oldPersistent, mapped);
        oldPersistent.clear();
        emit layoutChanged({}, QAbstractItemModel::VerticalSortHint);
    }
private:
    QModelIndexList oldPersistent;
};
