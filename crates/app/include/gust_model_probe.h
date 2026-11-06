#pragma once
#include <QtCore/QString>
#include <memory>
class DirModel;
std::unique_ptr<DirModel> gust_test_new_model();
void gust_test_attach_probe(DirModel &model);
void gust_test_remember_index(DirModel &model, std::int32_t row);
QString gust_test_probe_state(const DirModel &model);

void gust_test_publication_reads(DirModel &model, std::int32_t name, std::int32_t display, std::int32_t other);
