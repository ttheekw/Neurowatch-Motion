// CPU comparison runner. Upstream algorithms run unchanged in separate processes.
#include <opencv2/core.hpp>
#include <opencv2/core/ocl.hpp>
#include <opencv2/imgcodecs.hpp>
#include <opencv2/imgproc.hpp>
#include <opencv2/video/background_segm.hpp>
#include "BackgroundSubtractorPAWCS.h"
#include "BackgroundSubtractorSuBSENSE.h"
#include "mask_metrics.hpp"

#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/resource.h>
#include <time.h>
#include <vector>

extern "C" {
void* legacy_create(uint8_t sensitivity);
int legacy_observe(void*, const uint8_t*, int64_t, uint8_t*, float*, uint8_t*);
void legacy_destroy(void*);
}

namespace fs = std::filesystem;
using bgslibrary::algorithms::lbsp::BackgroundSubtractorPAWCS;
using bgslibrary::algorithms::lbsp::BackgroundSubtractorSuBSENSE;

static double cpu_ms() {
    timespec ts{};
    if (clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &ts))
        throw std::runtime_error("cannot read process CPU clock");
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static std::vector<std::string> split(const std::string& line) {
    std::vector<std::string> fields;
    size_t at = 0;
    for (;;) {
        const size_t end = line.find('\t', at);
        fields.push_back(line.substr(at, end == std::string::npos ? end : end - at));
        if (end == std::string::npos) return fields;
        at = end + 1;
    }
}

struct LegacyDeleter { void operator()(void* p) const { legacy_destroy(p); } };

int main(int argc, char** argv) try {
    // Positional arguments are constructed by compare.py, never by a shell.
    if (argc != 10) throw std::runtime_error(
        "expected: ALGORITHM FRAMES.tsv ROI OUTPUT_DIR WIDTH HEIGHT COLOR SEED SENSITIVITY");
    const std::string algorithm = argv[1];
    const int width = std::stoi(argv[5]), height = std::stoi(argv[6]);
    const bool color = std::string(argv[7]) == "color";
    const unsigned seed = std::stoul(argv[8]);
    const int sensitivity = std::stoi(argv[9]);
    if (width < 32 || height < 32 || sensitivity < 1 || sensitivity > 10)
        throw std::runtime_error("invalid dimensions/sensitivity");
    if (algorithm == "legacy" && (width != 320 || height != 180))
        throw std::runtime_error("legacy requires its production 320x180 input");
    fs::create_directories(argv[4]);
    const fs::path output = argv[4];
    std::ifstream manifest(argv[2]);
    if (!manifest) throw std::runtime_error("cannot open frame manifest");
    cv::Mat roi = cv::imread(argv[3], cv::IMREAD_GRAYSCALE);
    if (roi.empty()) throw std::runtime_error("cannot read spatial ROI");
    cv::compare(roi, 0, roi, cv::CMP_GT);
    if (!cv::countNonZero(roi)) throw std::runtime_error("empty spatial ROI");
    cv::setNumThreads(1);
    cv::ocl::setUseOpenCL(false);
    cv::setRNGSeed(static_cast<int>(seed));
    std::srand(seed);

    // Keep constructor and first-frame initialization costs observable separately.
    const double constructor_start = cpu_ms();
    std::unique_ptr<BackgroundSubtractorSuBSENSE> subsense;
    std::unique_ptr<BackgroundSubtractorPAWCS> pawcs;
    cv::Ptr<cv::BackgroundSubtractor> opencv;
    std::unique_ptr<void, LegacyDeleter> legacy;
    if (algorithm == "subsense") subsense = std::make_unique<BackgroundSubtractorSuBSENSE>();
    else if (algorithm == "pawcs") pawcs = std::make_unique<BackgroundSubtractorPAWCS>();
    else if (algorithm == "mog2") opencv = cv::createBackgroundSubtractorMOG2();
    else if (algorithm == "knn") opencv = cv::createBackgroundSubtractorKNN();
    else if (algorithm == "legacy") legacy.reset(legacy_create(sensitivity));
    else throw std::runtime_error("unknown algorithm: " + algorithm);
    const double constructor_cpu = cpu_ms() - constructor_start;

    std::ofstream csv(output / "frames.csv");
    if (!csv) throw std::runtime_error("cannot create result CSV");
    csv << "frame,time_ms,evaluated,initialization,preprocess_cpu_ms,model_cpu_ms,model_wall_ms,"
           "tp,fp,fn,tn,shadow_fp,shadow_pixels,gt_foreground,valid_pixels,score,legacy_score,legacy_active\n";
    csv << std::setprecision(12);
    std::string line;
    size_t frames = 0;
    double previous_ms = -1;
    while (std::getline(manifest, line)) {
        const auto f = split(line);
        if (f.size() != 5) throw std::runtime_error("malformed frame manifest");
        const int frame_id = std::stoi(f[0]);
        const double timestamp_ms = std::stod(f[1]);
        const bool evaluated = f[2] == "1";
        if (!std::isfinite(timestamp_ms) || timestamp_ms <= previous_ms)
            throw std::runtime_error("frame timestamps must strictly increase");
        previous_ms = timestamp_ms;
        cv::Mat original = cv::imread(f[3], cv::IMREAD_COLOR);
        if (original.empty() || original.size() != roi.size())
            throw std::runtime_error("missing input or changed input dimensions: " + f[3]);
        const double preprocess_start = cpu_ms();
        cv::Mat frame;
        cv::resize(original, frame, cv::Size(width, height), 0, 0, cv::INTER_AREA);
        if (!color || algorithm == "legacy") cv::cvtColor(frame, frame, cv::COLOR_BGR2GRAY);
        if (!frame.isContinuous()) frame = frame.clone();
        const double preprocess_cpu = cpu_ms() - preprocess_start;
        cv::Mat mask;
        float legacy_score = -1;
        uint8_t legacy_active = 0;
        const double start_cpu = cpu_ms();
        const auto start_wall = std::chrono::steady_clock::now();
        if (subsense) {
            if (!frames) subsense->initialize(frame, cv::Mat());
            subsense->apply(frame, mask);
        } else if (pawcs) {
            if (!frames) pawcs->initialize(frame, cv::Mat());
            pawcs->apply(frame, mask);
        } else if (opencv) opencv->apply(frame, mask);
        else {
            mask.create(180, 320, CV_8UC1);
            if (legacy_observe(legacy.get(), frame.data, std::llround(timestamp_ms),
                               mask.data, &legacy_score, &legacy_active))
                throw std::runtime_error("legacy detector rejected frame");
        }
        const double model_cpu = cpu_ms() - start_cpu;
        const double model_wall = std::chrono::duration<double, std::milli>(
            std::chrono::steady_clock::now() - start_wall).count();
        // OpenCV shadow labels (127) are not foreground. No extra morphology is added.
        cv::compare(mask, 255, mask, cv::CMP_EQ);
        cv::Mat prediction;
        cv::resize(mask, prediction, original.size(), 0, 0, cv::INTER_NEAREST);
        // Save native-size binary masks for independent scoring/visual review.
        std::ostringstream name;
        name << "bin" << std::setw(6) << std::setfill('0') << frame_id << ".png";
        if (!cv::imwrite((output / name.str()).string(), prediction))
            throw std::runtime_error("cannot write prediction mask");
        const double score = double(cv::countNonZero(prediction & roi)) / cv::countNonZero(roi);
        MaskCounts counts;
        if (evaluated) {
            // Ground truth is read only AFTER inference and is never passed to a model.
            const cv::Mat gt = cv::imread(f[4], cv::IMREAD_GRAYSCALE);
            if (gt.empty() || gt.size() != original.size())
                throw std::runtime_error("missing or mismatched ground truth: " + f[4]);
            counts = count_mask(gt, prediction, roi);
        }
        csv << frame_id << ',' << timestamp_ms << ',' << evaluated << ',' << (frames==0) << ','
            << preprocess_cpu << ',' << model_cpu << ',' << model_wall << ','
            << counts.tp << ',' << counts.fp << ',' << counts.fn << ',' << counts.tn << ',' << counts.shadow_fp << ','
            << counts.shadow_pixels << ',' << counts.tp+counts.fn << ','
            << counts.tp+counts.fp+counts.fn+counts.tn << ',' << score << ','
            << legacy_score << ',' << int(legacy_active) << '\n';
        if (!csv) throw std::runtime_error("failed writing result CSV");
        ++frames;
    }
    if (!frames) throw std::runtime_error("empty manifest");
    rusage usage{};
    if (getrusage(RUSAGE_SELF, &usage)) throw std::runtime_error("cannot read memory usage");
    std::ofstream metadata(output / "runtime.txt");
    metadata << "opencv=" << CV_VERSION << "\nfeatures=" << cv::getCPUFeaturesLine()
             << "\nthreads=" << cv::getNumThreads() << "\nopencl=" << cv::ocl::useOpenCL()
             << "\nseed=" << seed << "\npeak_process_rss_kib=" << usage.ru_maxrss
             << "\nconstructor_cpu_ms=" << constructor_cpu << '\n';
    if (!metadata) throw std::runtime_error("failed writing runtime metadata");
    std::cout << algorithm << ": processed " << frames << " frames\n";
    return 0;
} catch (const std::exception& e) {
    std::cerr << "motion-eval: " << e.what() << '\n';
    return 1;
}
